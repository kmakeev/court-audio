//! Оркестратор сборки экспортного пакета (этап 10.2, шаг 1): дешифровка/
//! склейка ([`super::audio`]) [+ FLAC, [`super::flac`]] → метки
//! (`markers.json`/`markers.txt`) → манифест копии ([`super::manifest`]) →
//! автономный HTML-плеер ([`super::html`]). Структура пакета — задокументирована
//! в `docs/export_package.md`.

use std::fs;
use std::path::{Path, PathBuf};

use super::audio::{join_mix_to_wav, join_track_to_wav};
use super::flac::encode_wav_to_flac;
use super::html::{self, PlayerData, PlayerTrackView};
use super::manifest::{build_copy_manifest, CopyFileEntry, CopyManifest};
use super::ExportError;
use crate::integrity::annotations::{self, AnnotationSnapshot, MarkerState, RoleSpanState};
use crate::integrity::hash;
use crate::player::timeline::Timeline;
use crate::store::manifest::ManifestStore;

/// Одна дорожка, включаемая в пакет.
pub struct PlannedTrack {
    pub track_id: u32,
    pub role: String,
    pub label: String,
    pub timeline: Timeline,
}

/// Состав пакета: отдельный файл на дорожку либо один сведённый микс.
pub enum PackageTrackPlan {
    Separate(Vec<PlannedTrack>),
    Mix(Vec<PlannedTrack>),
}

/// Формат аудиофайлов пакета.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    WavPcm,
    Flac,
}

impl PackageFormat {
    fn code(self) -> &'static str {
        match self {
            PackageFormat::WavPcm => "wav_pcm",
            PackageFormat::Flac => "flac",
        }
    }
}

/// Запрос на сборку пакета.
pub struct BuildRequest<'a> {
    pub store: &'a ManifestStore,
    pub session_id: &'a str,
    /// Каталог назначения — уже выбранный вызывающим (папка/USB/временный
    /// каталог перед прожигом DVD); `package.rs` только наполняет его.
    pub package_dir: &'a Path,
    pub key: Option<[u8; 32]>,
    pub plan: PackageTrackPlan,
    pub format: PackageFormat,
    pub segment_hash: &'a str,
    pub hash_chain: bool,
    pub operator_id: &'a str,
    pub exported_at_unix_ms: u64,
    pub composition_detail: serde_json::Value,
    pub session_started_at_unix_ms: u64,
    pub adjudication_ref: Option<String>,
    pub seek_step_seconds: f32,
    pub playback_rates: Vec<f32>,
}

/// Итог сборки пакета.
pub struct BuildOutcome {
    pub files: Vec<CopyFileEntry>,
    pub manifest: CopyManifest,
    pub manifest_path: PathBuf,
    pub player_path: PathBuf,
    pub audio_dir: PathBuf,
}

/// Собрать экспортный пакет. `progress(stage, percent)` — для событий
/// `export_progress` в IPC-слое.
pub fn build_package(
    req: BuildRequest,
    mut progress: impl FnMut(&str, u8),
) -> Result<BuildOutcome, ExportError> {
    let audio_dir = req.package_dir.join("audio");
    fs::create_dir_all(&audio_dir)?;

    progress("joining", 10);
    let mut files: Vec<CopyFileEntry> = Vec::new();
    let mut player_tracks: Vec<PlayerTrackView> = Vec::new();
    let mut duration_ms: u64 = 0;

    match &req.plan {
        PackageTrackPlan::Separate(tracks) => {
            for t in tracks {
                let label_source = if t.label.trim().is_empty() { &t.role } else { &t.label };
                let stem = sanitize_filename(label_source, t.track_id);
                let wav_path = audio_dir.join(format!("{stem}.wav"));
                let joined = join_track_to_wav(&t.timeline, req.key.as_ref(), &wav_path)?;
                if duration_ms == 0 && joined.spec.sample_rate > 0 {
                    duration_ms = joined.frames * 1000 / joined.spec.sample_rate as u64;
                }
                let rel_name = finalize_format(&wav_path, &audio_dir, &stem, req.format)?;
                files.push(file_entry(
                    &audio_dir.join(&rel_name),
                    &rel_name,
                    Some(t.track_id),
                    Some(t.role.clone()),
                )?);
                player_tracks.push(PlayerTrackView {
                    file: format!("audio/{rel_name}"),
                    label: t.label.clone(),
                    role: t.role.clone(),
                });
            }
        }
        PackageTrackPlan::Mix(tracks) => {
            let timelines: Vec<&Timeline> = tracks.iter().map(|t| &t.timeline).collect();
            let stem = "mix".to_string();
            let wav_path = audio_dir.join(format!("{stem}.wav"));
            let joined = join_mix_to_wav(&timelines, req.key.as_ref(), &wav_path)?;
            if joined.spec.sample_rate > 0 {
                duration_ms = joined.frames * 1000 / joined.spec.sample_rate as u64;
            }
            let rel_name = finalize_format(&wav_path, &audio_dir, &stem, req.format)?;
            files.push(file_entry(&audio_dir.join(&rel_name), &rel_name, None, Some("mix".to_string()))?);
            player_tracks.push(PlayerTrackView {
                file: format!("audio/{rel_name}"),
                label: "Микс всех дорожек".to_string(),
                role: "mix".to_string(),
            });
        }
    }

    progress("annotations", 50);
    let annotation_log = req.store.get_annotations(req.session_id)?;
    let snapshot = annotations::fold(&annotation_log);
    // Смещения меток/интервалов — по WALL-CLOCK оси сессии (с паузами), а
    // склеенный файл — по оси ФРЕЙМОВ (паузы вырезаны). Спроецировать одно на
    // другое, иначе метка после паузы указывает за фактический конец аудио и
    // плеер упирается в конец потока (R-011). Опора — первая дорожка пакета
    // (все дорожки сессии стартуют синхронно, общий клок пауз).
    let snapshot = match reference_timeline(&req.plan) {
        Some(tl) => project_annotations(&snapshot, tl, req.session_started_at_unix_ms),
        None => snapshot,
    };
    write_markers_files(req.package_dir, &snapshot)?;

    progress("player", 75);
    let player_data = PlayerData {
        session_id: req.session_id.to_string(),
        started_at_unix_ms: req.session_started_at_unix_ms,
        adjudication_ref: req.adjudication_ref.clone(),
        tracks: player_tracks,
        markers: snapshot.markers.clone(),
        role_spans: snapshot.role_spans.clone(),
        duration_ms,
        seek_step_seconds: req.seek_step_seconds,
        playback_rates: req.playback_rates.clone(),
    };
    let player_html = html::render(&player_data)?;
    let player_path = req.package_dir.join("player.html");
    fs::write(&player_path, player_html)?;

    progress("manifest", 90);
    let manifest = build_copy_manifest(
        req.store,
        req.session_id,
        req.segment_hash,
        req.hash_chain,
        req.operator_id,
        req.exported_at_unix_ms,
        req.composition_detail,
        req.format.code(),
        files.clone(),
    )?;
    let manifest_path = req.package_dir.join("manifest.json");
    fs::write(&manifest_path, manifest.to_json_pretty()?)?;

    progress("done", 100);

    Ok(BuildOutcome {
        files,
        manifest,
        manifest_path,
        player_path,
        audio_dir,
    })
}

/// Дорожка-опора для проекции разметки на ось фреймов: первая дорожка пакета
/// (в `Separate`/`Track` — единственная воспроизводимая на слайдере, в `Mix` —
/// первая из сводимых; все дорожки сессии синхронны по единому клоку пауз).
fn reference_timeline(plan: &PackageTrackPlan) -> Option<&Timeline> {
    match plan {
        PackageTrackPlan::Separate(tracks) | PackageTrackPlan::Mix(tracks) => {
            tracks.first().map(|t| &t.timeline)
        }
    }
}

/// Спроецировать смещения меток/интервалов с WALL-CLOCK оси сессии (с паузами)
/// на ось ФРЕЙМОВ склеенного файла (паузы вырезаны). Единица правды — реальные
/// сэмплы: [`Timeline::frame_for_marker`] клэмпит метку из паузы к последнему
/// записанному фрейму, а метку за концом — к последнему фрейму дорожки, поэтому
/// спроецированное смещение всегда попадает внутрь файла (плеер не встаёт на
/// конце потока). `offset_samples`/`offset_ms` переписываются на ось файла;
/// прочие поля (id/категория/автор) сохраняются.
fn project_annotations(
    snapshot: &AnnotationSnapshot,
    timeline: &Timeline,
    session_started_at_unix_ms: u64,
) -> AnnotationSnapshot {
    let rate = timeline.sample_rate_hz;
    let to_axis = |offset_ms: u64| -> (u64, u64) {
        let frame = timeline.frame_for_marker(session_started_at_unix_ms, offset_ms);
        (frame, frames_to_ms(frame, rate))
    };
    let markers = snapshot
        .markers
        .iter()
        .map(|m| {
            let (frame, ms) = to_axis(m.offset_ms);
            MarkerState {
                offset_samples: frame,
                offset_ms: ms,
                ..m.clone()
            }
        })
        .collect();
    let role_spans = snapshot
        .role_spans
        .iter()
        .map(|s| {
            let (start_frame, start_ms) = to_axis(s.start_offset_ms);
            let (end_frame, end_ms) = match s.end_offset_ms {
                Some(end) => {
                    let (f, m) = to_axis(end);
                    (Some(f), Some(m))
                }
                None => (None, None),
            };
            RoleSpanState {
                start_offset_samples: start_frame,
                start_offset_ms: start_ms,
                end_offset_samples: end_frame,
                end_offset_ms: end_ms,
                ..s.clone()
            }
        })
        .collect();
    AnnotationSnapshot { markers, role_spans }
}

/// Число фреймов → миллисекунды (усечение вниз): спроецированный офсет строго
/// меньше `duration_ms`, поэтому клик по метке не упирается в самый конец файла.
fn frames_to_ms(frames: u64, sample_rate_hz: u32) -> u64 {
    if sample_rate_hz == 0 {
        return 0;
    }
    frames * 1000 / sample_rate_hz as u64
}

/// Если формат — FLAC, перекодировать промежуточный WAV и удалить его;
/// иначе оставить WAV как финальный файл. Возвращает имя файла (без каталога).
fn finalize_format(
    wav_path: &Path,
    audio_dir: &Path,
    stem: &str,
    format: PackageFormat,
) -> Result<String, ExportError> {
    match format {
        PackageFormat::WavPcm => Ok(format!("{stem}.wav")),
        PackageFormat::Flac => {
            let flac_path = audio_dir.join(format!("{stem}.flac"));
            encode_wav_to_flac(wav_path, &flac_path)?;
            fs::remove_file(wav_path)?;
            Ok(format!("{stem}.flac"))
        }
    }
}

fn file_entry(
    path: &Path,
    rel_name: &str,
    track_id: Option<u32>,
    role: Option<String>,
) -> Result<CopyFileEntry, ExportError> {
    let sha256 = hash::sha256_file(path).map_err(|e| ExportError::Io(e.to_string()))?;
    let size_bytes = fs::metadata(path)?.len();
    Ok(CopyFileEntry {
        name: format!("audio/{rel_name}"),
        sha256,
        size_bytes,
        track_id,
        role,
    })
}

fn write_markers_files(package_dir: &Path, snapshot: &AnnotationSnapshot) -> Result<(), ExportError> {
    let json_path = package_dir.join("markers.json");
    fs::write(&json_path, serde_json::to_string_pretty(snapshot)?)?;

    let mut items: Vec<(u64, String)> = Vec::new();
    for m in &snapshot.markers {
        let line = match &m.comment {
            Some(c) => format!("{}  [{}] {}", format_clock(m.offset_ms), m.category, c),
            None => format!("{}  [{}]", format_clock(m.offset_ms), m.category),
        };
        items.push((m.offset_ms, line));
    }
    for s in &snapshot.role_spans {
        let end = s.end_offset_ms.map(format_clock).unwrap_or_else(|| "…".to_string());
        items.push((
            s.start_offset_ms,
            format!("{}..{}  Роль: {}", format_clock(s.start_offset_ms), end, s.role),
        ));
    }
    items.sort_by_key(|(ms, _)| *ms);
    let text = items.into_iter().map(|(_, line)| line).collect::<Vec<_>>().join("\n");

    let txt_path = package_dir.join("markers.txt");
    fs::write(&txt_path, text)?;
    Ok(())
}

fn format_clock(total_ms: u64) -> String {
    let total_sec = total_ms / 1000;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// ASCII-безопасный слаг для имени файла/тома (Joliet/ISO9660-совместимость):
/// латиница/цифры/`-`/`_` в нижнем регистре, кириллица — упрощённая
/// транслитерация; пусто после очистки → `track_<id>`.
pub fn sanitize_filename(label_or_role: &str, fallback_track_id: u32) -> String {
    let translit = transliterate(label_or_role);
    let slug: String = translit
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('_');
    // Схлопнуть повторные `_`, оставшиеся от неASCII-символов подряд.
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_underscore = false;
    for c in trimmed.chars() {
        if c == '_' {
            if !prev_underscore {
                out.push(c);
            }
            prev_underscore = true;
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    if out.is_empty() {
        format!("track_{fallback_track_id}")
    } else {
        out
    }
}

/// Минимальная транслитерация кириллицы для имён файлов (не полноценный
/// ГОСТ-стандарт — только устойчивая передача звучания для читаемого слага).
fn transliterate(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mapped: &str = match c {
                'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d",
                'е' | 'ё' => "e", 'ж' => "zh", 'з' => "z", 'и' | 'й' => "i",
                'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
                'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u",
                'ф' => "f", 'х' => "h", 'ц' => "c", 'ч' => "ch", 'ш' => "sh",
                'щ' => "sch", 'ъ' | 'ь' => "", 'ы' => "y", 'э' => "e",
                'ю' => "yu", 'я' => "ya",
                'А' => "A", 'Б' => "B", 'В' => "V", 'Г' => "G", 'Д' => "D",
                'Е' | 'Ё' => "E", 'Ж' => "Zh", 'З' => "Z", 'И' | 'Й' => "I",
                'К' => "K", 'Л' => "L", 'М' => "M", 'Н' => "N", 'О' => "O",
                'П' => "P", 'Р' => "R", 'С' => "S", 'Т' => "T", 'У' => "U",
                'Ф' => "F", 'Х' => "H", 'Ц' => "C", 'Ч' => "Ch", 'Ш' => "Sh",
                'Щ' => "Sch", 'Ъ' | 'Ь' => "", 'Ы' => "Y", 'Э' => "E",
                'Ю' => "Yu", 'Я' => "Ya",
                _ => {
                    // Однобайтовые ASCII-символы возвращаем как есть; прочее
                    // (не кириллица/не ASCII) сворачивается в `_` на следующем
                    // шаге sanitize_filename.
                    return c.to_string();
                }
            };
            mapped.to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::annotations::{
        build_annotation_chain, AnnotationAction, AnnotationRecord,
    };
    use crate::store::manifest::{SegmentRecord, SessionRecord, TrackRecord};
    use hound::{SampleFormat, WavSpec, WavWriter};

    fn write_wav(path: &Path, rate: u32, samples: &[i32]) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn sanitize_filename_transliterates_and_strips_unsafe_chars() {
        assert_eq!(sanitize_filename("Судья", 0), "sudya");
        assert_eq!(sanitize_filename("Защита №2", 1), "zaschita_2");
    }

    #[test]
    fn sanitize_filename_falls_back_to_track_id_when_empty() {
        assert_eq!(sanitize_filename("", 3), "track_3");
        assert_eq!(sanitize_filename("!!!", 4), "track_4");
    }

    fn seed_two_track_session(tmp: &Path) -> (ManifestStore, PathBuf) {
        let dir = tmp.join("sess-1");
        std::fs::create_dir_all(&dir).unwrap();
        let store = ManifestStore::in_memory().unwrap();
        store
            .insert_session(&SessionRecord::new(
                "sess-1",
                dir.to_string_lossy().as_ref(),
                1_700_000_000_000,
                "station-1",
                "operator-7",
                8_000,
                1,
                16,
            ))
            .unwrap();

        for (tid, role) in [(0u32, "judge"), (1u32, "defense")] {
            store
                .insert_track(
                    "sess-1",
                    &TrackRecord {
                        track_id: tid,
                        role: role.to_string(),
                        label: role.to_string(),
                        source_device: None,
                        source_channel: tid as u16,
                        final_chain_link: None,
                    },
                )
                .unwrap();
            let wav_path = dir.join(format!("t{tid}.wav"));
            write_wav(&wav_path, 8_000, &[1, 2, 3]);
            store
                .append_segment(
                    "sess-1",
                    &SegmentRecord {
                        track_id: tid,
                        index: 1,
                        path: wav_path.to_string_lossy().into_owned(),
                        started_at_unix_ms: 1_700_000_000_000,
                        frames: 3,
                        size_bytes: 6,
                        sha256: crate::integrity::hash::sha256_file(&wav_path).unwrap(),
                        chain_link: "link1".to_string(),
                    },
                )
                .unwrap();
        }

        let rec = AnnotationRecord {
            seq: 1,
            action: AnnotationAction::MarkerAdded,
            target_id: "m1".to_string(),
            category: Some("Инцидент".to_string()),
            role: None,
            comment: Some("шум в зале".to_string()),
            offset_samples: 0,
            offset_ms: 0,
            operator_id: "op-7".to_string(),
            at_unix_ms: 1_700_000_000_000,
            chain_link: String::new(),
        };
        let chain = build_annotation_chain(std::slice::from_ref(&rec));
        let mut rec = rec;
        rec.chain_link = chain.into_iter().next().unwrap();
        store.append_annotation("sess-1", &rec).unwrap();

        (store, dir)
    }

    fn planned_tracks(store: &ManifestStore) -> Vec<PlannedTrack> {
        let records = store.get_tracks("sess-1").unwrap();
        let segments = store.get_segments("sess-1").unwrap();
        records
            .into_iter()
            .map(|t| PlannedTrack {
                track_id: t.track_id,
                role: t.role.clone(),
                label: t.label.clone(),
                timeline: Timeline::build(&segments, t.track_id, 8_000),
            })
            .collect()
    }

    #[test]
    fn build_package_writes_manifest_markers_and_player_html() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _session_dir) = seed_two_track_session(tmp.path());
        let out_dir = tmp.path().join("export-out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let req = BuildRequest {
            store: &store,
            session_id: "sess-1",
            package_dir: &out_dir,
            key: None,
            plan: PackageTrackPlan::Separate(planned_tracks(&store)),
            format: PackageFormat::WavPcm,
            segment_hash: "sha256",
            hash_chain: true,
            operator_id: "op-7",
            exported_at_unix_ms: 1_700_000_100_000,
            composition_detail: serde_json::json!({"kind": "all_tracks"}),
            session_started_at_unix_ms: 1_700_000_000_000,
            adjudication_ref: None,
            seek_step_seconds: 15.0,
            playback_rates: vec![1.0],
        };

        let outcome = build_package(req, |_, _| {}).unwrap();

        assert_eq!(outcome.files.len(), 2);
        assert!(outcome.manifest_path.exists());
        assert!(outcome.player_path.exists());
        assert!(out_dir.join("markers.json").exists());
        assert!(out_dir.join("markers.txt").exists());

        let manifest_json = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        let back: CopyManifest = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(back.files.len(), 2);

        let markers_txt = std::fs::read_to_string(out_dir.join("markers.txt")).unwrap();
        assert!(markers_txt.contains("Инцидент"));
        assert!(markers_txt.contains("шум в зале"));

        let player_html = std::fs::read_to_string(&outcome.player_path).unwrap();
        assert!(player_html.contains("export-data"));
    }

    #[test]
    fn build_package_projects_annotations_onto_frame_axis_after_pause() {
        // R-011: сессия с паузой между сегментами; метка и интервал роли
        // поставлены ПОСЛЕ паузы (wall-clock офсет много больше фактической
        // длительности склеенного файла). После проекции их смещения обязаны
        // попадать ВНУТРЬ файла — иначе переход к ним в плеере упирается в
        // конец потока и «Играть» перестаёт возобновлять (исходный кейс).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sess-pause");
        std::fs::create_dir_all(&dir).unwrap();
        let store = ManifestStore::in_memory().unwrap();

        let rate = 8_000u32;
        let t0 = 1_700_000_000_000u64;
        store
            .insert_session(&SessionRecord::new(
                "sess-pause",
                dir.to_string_lossy().as_ref(),
                t0,
                "station-1",
                "op-7",
                rate,
                1,
                16,
            ))
            .unwrap();
        store
            .insert_track(
                "sess-pause",
                &TrackRecord {
                    track_id: 0,
                    role: "judge".to_string(),
                    label: "judge".to_string(),
                    source_device: None,
                    source_channel: 0,
                    final_chain_link: None,
                },
            )
            .unwrap();

        // seg1: 1 с в начале сессии; пауза 100 с; seg2: ещё 1 с.
        let frames = rate as u64; // ровно 1 секунда
        let samples: Vec<i32> = vec![0; frames as usize];
        for (index, started) in [(1u32, t0), (2u32, t0 + 101_000)] {
            let wav_path = dir.join(format!("seg-{index}.wav"));
            write_wav(&wav_path, rate, &samples);
            store
                .append_segment(
                    "sess-pause",
                    &SegmentRecord {
                        track_id: 0,
                        index,
                        path: wav_path.to_string_lossy().into_owned(),
                        started_at_unix_ms: started,
                        frames,
                        size_bytes: frames * 2,
                        sha256: crate::integrity::hash::sha256_file(&wav_path).unwrap(),
                        chain_link: format!("link{index}"),
                    },
                )
                .unwrap();
        }

        // Метка на 0.5 с ВНУТРИ второго сегмента и интервал роли, открытый там
        // же — оба на wall-clock оси (офсеты > 100 с), склеенный файл — 2 с.
        let marker_wall_ms = 101_500u64;
        let role_wall_ms = 101_000u64;
        let mk = |seq: u32, action: AnnotationAction, target: &str, off_ms: u64| AnnotationRecord {
            seq,
            action,
            target_id: target.to_string(),
            category: Some("Инцидент".to_string()),
            role: Some("judge".to_string()),
            comment: None,
            offset_samples: off_ms * rate as u64 / 1000,
            offset_ms: off_ms,
            operator_id: "op-7".to_string(),
            at_unix_ms: t0 + off_ms,
            chain_link: String::new(),
        };
        let mut recs = vec![
            mk(1, AnnotationAction::MarkerAdded, "m1", marker_wall_ms),
            mk(2, AnnotationAction::RoleStarted, "r1", role_wall_ms),
        ];
        let chain = build_annotation_chain(&recs);
        for (r, link) in recs.iter_mut().zip(chain) {
            r.chain_link = link;
        }
        for r in &recs {
            store.append_annotation("sess-pause", r).unwrap();
        }

        let segments = store.get_segments("sess-pause").unwrap();
        let timeline = Timeline::build(&segments, 0, rate);
        let out_dir = tmp.path().join("export-out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let req = BuildRequest {
            store: &store,
            session_id: "sess-pause",
            package_dir: &out_dir,
            key: None,
            plan: PackageTrackPlan::Separate(vec![PlannedTrack {
                track_id: 0,
                role: "judge".to_string(),
                label: "judge".to_string(),
                timeline,
            }]),
            format: PackageFormat::WavPcm,
            segment_hash: "sha256",
            hash_chain: true,
            operator_id: "op-7",
            exported_at_unix_ms: t0 + 200_000,
            composition_detail: serde_json::json!({"kind": "all_tracks"}),
            session_started_at_unix_ms: t0,
            adjudication_ref: None,
            seek_step_seconds: 15.0,
            playback_rates: vec![1.0],
        };
        build_package(req, |_, _| {}).unwrap();

        // Склеенный файл непрерывен и длится ровно суммарные сэмплы (2 с).
        let wav = out_dir.join("audio/judge.wav");
        let joined = hound::WavReader::open(&wav).unwrap();
        assert_eq!(joined.duration(), 2 * frames as u32);
        let duration_ms = 2 * frames * 1000 / rate as u64; // 2000

        // markers.json несёт спроецированные (ось файла) смещения — внутри файла.
        let snap: AnnotationSnapshot =
            serde_json::from_str(&std::fs::read_to_string(out_dir.join("markers.json")).unwrap())
                .unwrap();
        // 0.5 с во втором сегменте = 1.5 с на оси файла (первая 1 с + 0.5 с).
        assert_eq!(snap.markers[0].offset_ms, 1_500);
        assert!(snap.markers[0].offset_ms < duration_ms);
        // Начало интервала = стык на 1.0 с оси файла (не 101 с wall-clock).
        assert_eq!(snap.role_spans[0].start_offset_ms, 1_000);
        assert!(snap.role_spans[0].start_offset_ms < duration_ms);
    }

    #[test]
    fn build_package_reports_progress_stages_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _session_dir) = seed_two_track_session(tmp.path());
        let out_dir = tmp.path().join("export-out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let req = BuildRequest {
            store: &store,
            session_id: "sess-1",
            package_dir: &out_dir,
            key: None,
            plan: PackageTrackPlan::Mix(planned_tracks(&store)),
            format: PackageFormat::WavPcm,
            segment_hash: "sha256",
            hash_chain: true,
            operator_id: "op-7",
            exported_at_unix_ms: 1_700_000_100_000,
            composition_detail: serde_json::json!({"kind": "mix"}),
            session_started_at_unix_ms: 1_700_000_000_000,
            adjudication_ref: Some("№ 1-1/2026".to_string()),
            seek_step_seconds: 15.0,
            playback_rates: vec![1.0],
        };

        let mut stages: Vec<(String, u8)> = Vec::new();
        build_package(req, |stage, pct| stages.push((stage.to_string(), pct))).unwrap();

        let names: Vec<&str> = stages.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(names, vec!["joining", "annotations", "player", "manifest", "done"]);
        let percents: Vec<u8> = stages.iter().map(|(_, p)| *p).collect();
        assert!(percents.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*percents.last().unwrap(), 100);
    }
}

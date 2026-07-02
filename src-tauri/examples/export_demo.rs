//! Разовый (не для поставки) прогон реального пути экспорта (этап 10.2):
//! синтетическая двухдорожечная сессия с зашифрованными at-rest сегментами →
//! `export::package::build_package` → пакет на диске для ручной проверки.
//! Запуск: `cargo run --example export_demo`.

use std::path::{Path, PathBuf};

use court_audio_lib::export::manifest::CopyManifest;
use court_audio_lib::export::package::{
    build_package, BuildRequest, PackageFormat, PackageTrackPlan, PlannedTrack,
};
use court_audio_lib::integrity::annotations::{AnnotationAction, AnnotationRecord};
use court_audio_lib::integrity::hash;
use court_audio_lib::player::timeline::Timeline;
use court_audio_lib::store::case_binding::AdjudicationRef;
use court_audio_lib::store::crypto;
use court_audio_lib::store::manifest::{ManifestStore, SegmentRecord, SessionRecord, TrackRecord};

const SAMPLE_RATE: u32 = 44_100;
const KEY: [u8; 32] = [0x42; 32];

fn write_wav_segment(path: &Path, seconds: f32, freq_hz: f32, amplitude: i16) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (SAMPLE_RATE as f32 * seconds) as u32;
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let v = (amplitude as f32 * (2.0 * std::f32::consts::PI * freq_hz * t).sin()) as i16;
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

fn seed_track(store: &ManifestStore, session_dir: &Path, session_id: &str, track_id: u32, role: &str, label: &str, freq_hz: f32) {
    store
        .insert_track(
            session_id,
            &TrackRecord {
                track_id,
                role: role.to_string(),
                label: label.to_string(),
                source_device: None,
                source_channel: track_id as u16,
                final_chain_link: None,
            },
        )
        .unwrap();

    // Два сегмента по ~1.5с — реалистичная синусоида, не тишина/заглушка.
    let mut prev_hash: Option<String> = None;
    for idx in 1..=2u32 {
        let plain_path = session_dir.join(format!("t{track_id}-seg{idx:04}.wav"));
        write_wav_segment(&plain_path, 1.5, freq_hz + idx as f32 * 20.0, 8_000);
        let fin = crypto::finalize_segment(&plain_path, Some(&KEY), true).unwrap();
        let seg_hash = fin.content_sha256.clone();
        let link = hash::chain_link(prev_hash.as_deref(), &seg_hash);
        prev_hash = Some(link.clone());

        let frames = (SAMPLE_RATE as f32 * 1.5) as u64;
        store
            .append_segment(
                session_id,
                &SegmentRecord {
                    track_id,
                    index: idx,
                    path: fin.stored_path.to_string_lossy().into_owned(),
                    started_at_unix_ms: 1_700_000_000_000 + ((idx - 1) as u64) * 1_500,
                    frames,
                    size_bytes: fin.content_size_bytes,
                    sha256: seg_hash,
                    chain_link: link,
                },
            )
            .unwrap();
    }
    store
        .set_track_final_chain_link(session_id, track_id, prev_hash.as_deref().unwrap())
        .unwrap();
}

fn main() {
    let root = PathBuf::from("/tmp/court-audio-export-demo");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let session_id = "demo-session-1";
    let session_dir = root.join("station").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();

    let store = ManifestStore::open(&root.join("manifest.sqlite")).unwrap();
    store
        .insert_session(&SessionRecord::new(
            session_id,
            session_dir.to_string_lossy().as_ref(),
            1_700_000_000_000,
            "station-demo",
            "operator-demo",
            SAMPLE_RATE,
            1,
            16,
        ))
        .unwrap();
    let ar = AdjudicationRef::manual("№ 1-777/2026", Some("Иванов И.И.".to_string())).unwrap();
    store
        .set_adjudication_ref(session_id, Some(&ar.to_json().unwrap()))
        .unwrap();

    seed_track(&store, &session_dir, session_id, 0, "judge", "Судья", 220.0);
    seed_track(&store, &session_dir, session_id, 1, "defense", "Защита", 330.0);

    // Живая разметка: закладка + интервал роли — та же схема, что в остальных тестах.
    let mut recs = vec![
        AnnotationRecord {
            seq: 1,
            action: AnnotationAction::MarkerAdded,
            target_id: "m1".into(),
            category: Some("Инцидент".into()),
            role: None,
            comment: Some("шум в зале".into()),
            offset_samples: 44_100,
            offset_ms: 1_000,
            operator_id: "operator-demo".into(),
            at_unix_ms: 1_700_000_001_000,
            chain_link: String::new(),
        },
        AnnotationRecord {
            seq: 2,
            action: AnnotationAction::RoleStarted,
            target_id: "r1".into(),
            category: None,
            role: Some("judge".into()),
            comment: None,
            offset_samples: 88_200,
            offset_ms: 2_000,
            operator_id: "operator-demo".into(),
            at_unix_ms: 1_700_000_002_000,
            chain_link: String::new(),
        },
    ];
    let chain = court_audio_lib::integrity::annotations::build_annotation_chain(&recs);
    for (r, link) in recs.iter_mut().zip(chain) {
        r.chain_link = link;
    }
    for r in &recs {
        store.append_annotation(session_id, r).unwrap();
    }

    let tracks = store.get_tracks(session_id).unwrap();
    let segments = store.get_segments(session_id).unwrap();
    let planned: Vec<PlannedTrack> = tracks
        .iter()
        .map(|t| PlannedTrack {
            track_id: t.track_id,
            role: t.role.clone(),
            label: t.label.clone(),
            timeline: Timeline::build(&segments, t.track_id, SAMPLE_RATE),
        })
        .collect();

    for (format, format_name) in [(PackageFormat::WavPcm, "wav"), (PackageFormat::Flac, "flac")] {
        let out_dir = root.join(format!("export_{format_name}"));
        std::fs::create_dir_all(&out_dir).unwrap();

        let req = BuildRequest {
            store: &store,
            session_id,
            package_dir: &out_dir,
            key: Some(KEY),
            plan: PackageTrackPlan::Separate(
                planned
                    .iter()
                    .map(|p| PlannedTrack {
                        track_id: p.track_id,
                        role: p.role.clone(),
                        label: p.label.clone(),
                        timeline: p.timeline.clone(),
                    })
                    .collect(),
            ),
            format,
            segment_hash: "sha256",
            hash_chain: true,
            operator_id: "operator-demo",
            exported_at_unix_ms: 1_700_000_100_000,
            composition_detail: serde_json::json!({"kind": "all_tracks"}),
            session_started_at_unix_ms: 1_700_000_000_000,
            adjudication_ref: Some(ar.to_json().unwrap()),
            seek_step_seconds: 15.0,
            playback_rates: vec![0.5, 1.0, 1.5, 2.0],
        };

        let outcome = build_package(req, |stage, pct| println!("[{format_name}] {stage} {pct}%")).unwrap();

        println!("\n=== Формат {format_name} ===");
        println!("Пакет: {}", out_dir.display());
        println!("Манифест: {}", outcome.manifest_path.display());
        println!("Плеер: {}", outcome.player_path.display());

        // Независимая проверка: пересчитать SHA-256 каждого файла с диска и
        // сверить с manifest.json — именно то, что должен сделать получатель
        // копии (критерий приёмки «целостность копии проверяема получателем»).
        let manifest_json = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        let manifest: CopyManifest = serde_json::from_str(&manifest_json).unwrap();
        for f in &manifest.files {
            let path = out_dir.join(&f.name);
            let actual = hash::sha256_file(&path).unwrap();
            let ok = actual == f.sha256;
            println!(
                "  {} — {} байт — manifest {}… — актуальный {}… — {}",
                f.name,
                f.size_bytes,
                &f.sha256[..12],
                &actual[..12],
                if ok { "СОВПАДАЕТ" } else { "!!! НЕ СОВПАДАЕТ !!!" }
            );
            assert!(ok, "хеш файла {} разошёлся с манифестом", f.name);
        }
        println!(
            "Метки в манифесте: {} | интервалы ролей: {}",
            manifest.recording.annotations.markers.len(),
            manifest.recording.annotations.role_spans.len(),
        );
    }

    println!("\nГотово. Открыть плееры вручную:");
    println!("  open {}/export_wav/player.html", root.display());
    println!("  open {}/export_flac/player.html", root.display());
}

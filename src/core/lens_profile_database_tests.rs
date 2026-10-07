// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::camera_database::Selection;
use serde_json::{Value, json};
use std::io::Write;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("gyroflow-catalogue-{}-{}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn bundle(&self, catalogue: Value, profiles: Vec<(String, Value)>) {
        let mut entries = vec![("__version".to_owned(), json!(42)), ("__camera_database".to_owned(), catalogue)];
        entries.extend(profiles);
        let mut data = Vec::new();
        ciborium::into_writer(&entries, &mut data).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&data).unwrap();
        std::fs::write(self.0.join("profiles.cbor.gz"), encoder.finish().unwrap()).unwrap();
    }
    fn load(&self) -> LensProfileDatabase {
        let mut db = LensProfileDatabase::default();
        db.load_directories(self.0.clone(), self.0.join("absent"));
        db.prepare_list_for_ui();
        db
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn catalogue() -> Value {
    json!({"schema_version":1, "mounts":[], "cameras":[
        {"id":"future/camera2","brand":"Future","model":"Camera 2","aliases":["Cam II"],"profile_lenses":["prime"]}
    ], "lenses":[{"id":"prime","brand":"Maker","model":"Prime 50mm"}]})
}
fn profile(id: &str, author: &str, focal: f64) -> Value {
    json!({"camera_brand":"Future","camera_model":"Cam II","lens_model":"Maker Prime 50mm",
        "calibrator_version":"1.6.3", "calibrated_by":author, "identifier":id,
        "fps":30, "calib_dimension":{"w":1920,"h":1080},
        "fisheye_params":{"camera_matrix":[[focal,0,960],[0,focal,540],[0,0,1]],"distortion_coeffs":[0,0,0,0]}})
}
fn selection() -> Selection {
    Selection { camera_brand:"Future".into(), camera_model:"Camera 2".into(), lens_model:"Maker Prime 50mm".into() }
}

#[test]
fn browse_includes_loadable_compatible_lens_modes() {
    const BASE_ID: &str = "gopro-hero11black-wide-4000x3000@23976-no-eis";
    const SUPER_ID: &str = "gopro-hero11black-super-1920x1080@23976-no-eis";
    let fixture = Fixture::new();
    fixture.bundle(json!({
        "schema_version": 1,
        "mounts": [],
        "cameras": [{
            "id": "gopro/hero11black", "brand": "GoPro", "model": "Hero11 Black",
            "aliases": ["HERO11 Black"],
            "profile_lenses": ["gyroflow/gopro/hero11black/wide", "gyroflow/gopro/hero11black/super"]
        }],
        "lenses": [
            {"id": "gyroflow/gopro/hero11black/wide", "brand": "", "model": "Wide"},
            {"id": "gyroflow/gopro/hero11black/super", "brand": "", "model": "Super"}
        ]
    }), vec![("GoPro/GoPro_HERO11 Black_Wide_4by3.json".into(), json!({
        "camera_brand": "GoPro", "camera_model": "HERO11 Black", "lens_model": "Wide",
        "calibrator_version": "1.2.0", "calibrated_by": "Eddy", "official": true,
        "identifier": BASE_ID, "fps": 23.976,
        "calib_dimension": {"w": 4000, "h": 3000},
        "orig_dimension": {"w": 4000, "h": 3000},
        "fisheye_params": {
            "camera_matrix": [[1000, 0, 2000], [0, 1000, 1500], [0, 0, 1]],
            "distortion_coeffs": [0, 0, 0, 0]
        },
        "compatible_settings": [{
            "width": 1920, "height": 1080, "identifier": SUPER_ID, "fps": 23.976,
            "digital_lens": "gopro_superview", "lens_model": "Super"
        }]
    }))]);
    let db = fixture.load();
    let selection = Selection {
        camera_brand: "GoPro".into(), camera_model: "Hero11 Black".into(), lens_model: "Super".into()
    };

    let lenses = db.camera_database.options(&selection)["lenses"].as_array().unwrap().clone();
    assert!(lenses.iter().any(|lens| lens["name"] == "Super" && lens["has_profiles"] == true));
    let legacy_results = db.search("hero11", &std::collections::HashSet::new(), 0, 0);
    assert_eq!(legacy_results.len(), 1);
    assert_eq!(legacy_results[0].1, BASE_ID);
    assert_eq!(legacy_results[0].0, db.get_by_id(BASE_ID).unwrap().get_display_name());

    let manager = crate::StabilizationManager::default();
    *manager.lens_profile_db.write() = db;
    manager.load_lens_profile(SUPER_ID).unwrap();
    let loaded = manager.lens.read();
    // Database loading resolves the digital-lens adjustment before selection.
    assert_eq!((loaded.calib_dimension.w, loaded.calib_dimension.h), (1920, 1080));
    assert_eq!(loaded.digital_lens.as_deref(), Some("gopro_superview"));
    assert_eq!(loaded.lens_model, "Superview");
    assert!((loaded.fisheye_params.camera_matrix[0][0] - 360.0).abs() < 0.001);
    drop(loaded);

    let rows = manager.lens_profile_db.read().browse(&selection, false);
    let row = rows.as_array().unwrap().iter().find(|row| row["id"] == SUPER_ID)
        .expect("the selectable compatible lens mode should have a review row");
    assert_eq!((row["width"].as_u64(), row["height"].as_u64()), (Some(1920), Some(1080)));

    let mut replacement = LensProfileDatabase::default();
    replacement.set_from_db(manager.lens_profile_db.read().clone());
    replacement.prepare_list_for_ui();
    assert_eq!(replacement.browse(&selection, false), rows);
}

#[test]
fn browse_keeps_loaded_digital_mode_visible_with_distinct_catalogue_modes() {
    const COMPATIBLE_ID: &str = "hero7-compatible-super";
    const NATIVE_DIGITAL_ID: &str = "hero7-native-digital-super";
    const PLAIN_SUPER_ID: &str = "hero7-plain-super";
    const SUPERVIEW_ID: &str = "hero7-standalone-superview";
    let fixture = Fixture::new();
    let mut wide = profile("hero7-wide", "Wide author", 1000.0);
    wide["camera_brand"] = json!("GoPro");
    wide["camera_model"] = json!("HERO7 Black");
    wide["lens_model"] = json!("Wide");
    wide["calib_dimension"] = json!({"w": 4000, "h": 3000});
    wide["orig_dimension"] = wide["calib_dimension"].clone();
    wide["compatible_settings"] = json!([{
        "width": 1920, "height": 1080, "identifier": COMPATIBLE_ID, "fps": 30,
        "digital_lens": "gopro6_superview", "lens_model": "Super"
    }]);
    let mut plain_super = profile(PLAIN_SUPER_ID, "Super author", 900.0);
    plain_super["camera_brand"] = json!("GoPro");
    plain_super["camera_model"] = json!("HERO7 Black");
    plain_super["lens_model"] = json!("Super");
    let mut native_digital = plain_super.clone();
    native_digital["identifier"] = json!(NATIVE_DIGITAL_ID);
    native_digital["digital_lens"] = json!("gopro6_superview");
    native_digital["calib_dimension"] = json!({"w": 1440, "h": 1080});
    let mut superview = profile(SUPERVIEW_ID, "Superview author", 950.0);
    superview["camera_brand"] = json!("GoPro");
    superview["camera_model"] = json!("HERO7 Black");
    superview["lens_model"] = json!("Superview");
    // The actual bundled catalogue has separate owned Super and Superview IDs.
    fixture.bundle(serde_json::to_value(crate::camera_database::CameraDatabase::bundled()).unwrap(), vec![
        ("GoPro/hero7-wide.json".into(), wide),
        ("GoPro/hero7-super.json".into(), plain_super),
        ("GoPro/hero7-native-digital-super.json".into(), native_digital),
        ("GoPro/hero7-superview.json".into(), superview),
    ]);
    let db = fixture.load();
    let authored_selection = Selection {
        camera_brand: "GoPro".into(), camera_model: "HERO7 Black".into(), lens_model: "Super".into()
    };
    let camera = db.camera_database.camera("GoPro", "HERO7 Black");
    assert_ne!(db.camera_database.lens(camera, "Super").unwrap().id,
               db.camera_database.lens(camera, "Superview").unwrap().id);
    let authored_rows = db.browse(&authored_selection, false);
    let authored_rows = authored_rows.as_array().unwrap();
    assert!(authored_rows.iter().any(|row| row["id"] == COMPATIBLE_ID));
    assert!(authored_rows.iter().any(|row| row["id"] == NATIVE_DIGITAL_ID));
    assert!(authored_rows.iter().any(|row| row["id"] == PLAIN_SUPER_ID));
    assert!(!authored_rows.iter().any(|row| row["id"] == SUPERVIEW_ID));

    let manager = crate::StabilizationManager::default();
    *manager.lens_profile_db.write() = db;
    manager.load_lens_profile(COMPATIBLE_ID).unwrap();
    let loaded = manager.lens.read();
    assert_eq!(loaded.lens_model, "Superview");
    assert_eq!((loaded.calib_dimension.w, loaded.calib_dimension.h), (1920, 1080));
    // Match LensProfile.qml's selector prefill from the loaded profile metadata.
    let loaded_selection = Selection {
        camera_brand: loaded.camera_brand.clone(), camera_model: loaded.camera_model.clone(),
        lens_model: loaded.lens_model.clone()
    };
    drop(loaded);
    let loaded_rows = manager.lens_profile_db.read().browse(&loaded_selection, false);
    let loaded_rows = loaded_rows.as_array().unwrap();
    assert!(loaded_rows.iter().any(|row| row["id"] == COMPATIBLE_ID),
        "the loaded digital mode must remain visible under the selector's normalized lens name");
    assert!(loaded_rows.iter().any(|row| row["id"] == NATIVE_DIGITAL_ID),
        "a standalone digital profile must retain the same normalized-selection behavior");
    assert!(loaded_rows.iter().any(|row| row["id"] == SUPERVIEW_ID));
    assert!(!loaded_rows.iter().any(|row| row["id"] == PLAIN_SUPER_ID),
        "a distinct non-digital Super mode must not be conflated with Superview");
}

#[test]
fn normalized_digital_mode_match_does_not_expand_camera_suggestions() {
    let fixture = Fixture::new();
    let mut digital = profile("other-camera-digital", "Author", 900.0);
    digital["camera_model"] = json!("Camera 3");
    digital["lens_model"] = json!("Super");
    digital["digital_lens"] = json!("gopro_superview");
    digital["calib_dimension"] = json!({"w": 1440, "h": 1080});
    fixture.bundle(json!({
        "schema_version": 1, "mounts": [],
        "cameras": [
            {"id": "future/camera2", "brand": "Future", "model": "Camera 2",
             "mounts": ["fixture-mount"], "crop_factor": 1.0, "profile_lenses": ["super", "superview"]},
            {"id": "future/camera3", "brand": "Future", "model": "Camera 3",
             "mounts": ["fixture-mount"], "crop_factor": 1.0, "profile_lenses": ["super", "superview"]}
        ],
        "lenses": [
            {"id": "super", "brand": "", "model": "Super"},
            {"id": "superview", "brand": "", "model": "Superview"}
        ]
    }), vec![("Future/digital.json".into(), digital)]);
    let db = fixture.load();
    let camera2 = db.camera_database.camera("Future", "Camera 2").unwrap();
    let camera3 = db.camera_database.camera("Future", "Camera 3").unwrap();
    assert!(db.camera_database.compatible_cameras(camera2, camera3));
    let own = Selection {
        camera_brand: "Future".into(), camera_model: "Camera 3".into(), lens_model: "Superview".into()
    };
    assert_eq!(db.browse(&own, false).as_array().unwrap().len(), 1);
    let suggested = Selection { camera_model: "Camera 2".into(), ..own };
    assert!(db.browse(&suggested, true).as_array().unwrap().is_empty());
    let authored = Selection { lens_model: "Super".into(), ..suggested };
    let rows = db.browse(&authored, true);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["suggested"], true);
}

#[test]
fn browse_copy_name_collision_does_not_change_legacy_search_names() {
    let fixture = Fixture::new();
    let mut original = profile("original", "Author", 900.0);
    original["compatible_settings"] = json!([{
        "width": 1920, "height": 1080, "identifier": "same-display-copy", "fps": 30
    }]);
    fixture.bundle(catalogue(), vec![("Future/profile.json".into(), original)]);
    let db = fixture.load();

    let expected_name = db.get_by_id("original").unwrap().get_display_name();
    assert_eq!(db.get_by_id("same-display-copy").unwrap().get_display_name(), expected_name);
    let legacy_results = db.search("future", &std::collections::HashSet::new(), 0, 0);
    assert_eq!(legacy_results.len(), 1);
    assert_eq!(legacy_results[0].0, expected_name);
    assert_eq!(legacy_results[0].1, "original");

    let rows = db.browse(&selection(), false);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().find(|row| row["id"] == "original").unwrap()["name"].as_str(), Some(expected_name.as_str()));
    assert_ne!(rows.iter().find(|row| row["id"] == "same-display-copy").unwrap()["name"].as_str(), Some(expected_name.as_str()));
}

#[test]
fn updated_bundle_preserves_duplicate_identifier_submissions_and_clones() {
    let fixture = Fixture::new();
    fixture.bundle(catalogue(), vec![
        ("Future/one.json".into(), profile("same-id", "First", 900.0)),
        ("Future/two.json".into(), profile("same-id", "Second", 950.0)),
    ]);
    let db = fixture.load();
    assert_eq!(db.version, 42);
    assert_eq!(db.map.len(), 2);
    assert_eq!(db.get_by_id("same-id").unwrap().calibrated_by, "First");
    let rows = db.browse(&selection(), false);
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert!(rows.as_array().unwrap().iter().all(|r| db.get_by_id(r["id"].as_str().unwrap()).is_some()));
    assert_eq!(db.camera_database.cameras.len(), 1); // Updated catalogue replaces the bundled fallback.
    let mut replacement = LensProfileDatabase::default();
    replacement.set_from_db(db.clone());
    replacement.prepare_list_for_ui();
    assert_eq!(replacement.version, 42);
    assert_eq!(replacement.browse(&selection(), false), rows);
    assert_eq!(replacement.camera_database.camera("Future", "Cam II").unwrap().model, "Camera 2");
}

#[test]
fn unsupported_catalogue_does_not_discard_profiles_or_bundled_metadata() {
    let fixture = Fixture::new();
    let mut invalid = catalogue();
    invalid["schema_version"] = json!(999);
    fixture.bundle(invalid, vec![("Future/one.json".into(), profile("one", "Author", 900.0))]);
    let db = fixture.load();
    assert_eq!(db.map.len(), 1);
    assert!(db.camera_database.camera("Sony", "a6000").is_some());
    assert!(db.camera_database.camera("Future", "Cam II").is_some());
}

#[test]
fn catalogue_json_is_not_a_calibration_and_all_review_results_are_returned() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("camera_database.json"), catalogue().to_string()).unwrap();
    std::fs::create_dir_all(fixture.0.join("lensfun/data/db")).unwrap();
    std::fs::write(fixture.0.join("lensfun/data/db/lensfun-database.dtd"), "").unwrap();
    std::fs::write(fixture.0.join("lensfun/fixture.json"), profile("source-fixture", "Not a submitted profile", 900.0).to_string()).unwrap();
    fixture.bundle(catalogue(), (0..205).map(|i| (format!("Future/{i}.json"), profile(&format!("id-{i}"), &format!("Author {i}"), 900.0 + i as f64))).collect());
    let db = fixture.load();
    assert_eq!(db.map.len(), 205);
    assert_eq!(db.browse(&selection(), false).as_array().unwrap().len(), 205);
    let no_match = Selection { lens_model:"Nonexistent lens".into(), ..selection() };
    assert!(db.browse(&no_match, true).as_array().unwrap().is_empty());
}

#[test]
fn load_lens_profile_switches_between_duplicate_local_review_rows() {
    let fixture = Fixture::new();
    fixture.bundle(catalogue(), vec![]);
    for (file, author, focal) in [("one.json", "First", 900.0), ("two.json", "Second", 950.0)] {
        std::fs::write(fixture.0.join(file), profile("same-id", author, focal).to_string()).unwrap();
    }
    let db = fixture.load();
    let rows = db.browse(&selection(), false);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let alternate_id = rows.iter().map(|row| row["id"].as_str().unwrap())
        .find(|id| *id != "same-id").unwrap().to_owned();
    assert!(alternate_id.ends_with("#same-id"));
    let primary = db.get_by_id("same-id").unwrap().clone();
    let alternate = db.get_by_id(&alternate_id).unwrap().clone();
    assert_ne!(primary.calibrated_by, alternate.calibrated_by);

    let manager = crate::StabilizationManager::default();
    *manager.lens_profile_db.write() = db;
    for (id, expected) in [("same-id", &primary), (alternate_id.as_str(), &alternate), ("same-id", &primary)] {
        manager.load_lens_profile(id).unwrap();
        let loaded = manager.lens.read();
        assert_eq!(loaded.calibrated_by, expected.calibrated_by);
        assert_eq!(loaded.path_to_file, expected.path_to_file);
        assert_eq!(loaded.fisheye_params.camera_matrix, expected.fisheye_params.camera_matrix);
    }
}

#[test]
fn load_lens_profile_preserves_exact_ids_and_normalized_cache_fallback() {
    let fixture = Fixture::new();
    let raw_path = fixture.0.join("cached profile #1.json").to_string_lossy().to_string();
    let file_url = crate::filesystem::path_to_url(&raw_path);
    assert_ne!(raw_path, file_url);
    let ids = [raw_path.as_str(), file_url.as_str(), "Future/bundled.json",
        "C:/Profiles/two.json#same-id", r"C:\Profiles\two.json#same-id",
        r"\\server\profiles\two.json#same-id", "カメラ-profile"];
    fixture.bundle(catalogue(), ids.iter().enumerate().map(|(i, id)| {
        (id.to_string(), profile("", &format!("Cached {i}"), 900.0 + i as f64))
    }).collect());
    let manager = crate::StabilizationManager::default();
    *manager.lens_profile_db.write() = fixture.load();

    for (i, id) in ids.iter().enumerate() {
        manager.load_lens_profile(id).unwrap();
        assert_eq!(manager.lens.read().calibrated_by, format!("Cached {i}"), "ID: {id}");
    }
    // The raw path and file URL deliberately identify different cached profiles.
    // URL normalization is a fallback only when the exact key is absent.
    manager.lens_profile_db.write().map.remove(&raw_path);
    manager.load_lens_profile(&raw_path).unwrap();
    assert_eq!(manager.lens.read().calibrated_by, "Cached 1");
}

#[test]
fn load_lens_profile_keeps_file_and_inline_json_fallbacks() {
    let fixture = Fixture::new();
    let file = fixture.0.join("external profile #1.json");
    std::fs::write(&file, profile("disk-id", "Disk file", 920.0).to_string()).unwrap();
    let path = file.to_string_lossy().to_string();
    let file_url = crate::filesystem::path_to_url(&path);
    let manager = crate::StabilizationManager::default();

    for input in [path, file_url] {
        manager.load_lens_profile(&input).unwrap();
        assert_eq!(manager.lens.read().calibrated_by, "Disk file");
        assert!(manager.lens.read().official);
    }
    manager.load_lens_profile(&profile("inline-id", "Inline JSON", 970.0).to_string()).unwrap();
    assert_eq!(manager.lens.read().calibrated_by, "Inline JSON");
    assert_eq!(manager.lens.read().fisheye_params.camera_matrix[0][0], 970.0);
    assert!(manager.load_lens_profile("カメラ-not-a-file.json").is_err());
}

use super::*;

fn metadata(
    id: &str,
    name: &str,
    version: &str,
    published_at: &str,
    provides: &[&str],
) -> ExtensionMetadata {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "version": version,
        "authors": [],
        "repository": "https://example.com/repository",
        "published_at": published_at,
        "download_count": 0,
        "provides": provides,
    }))
    .expect("valid extension metadata")
}

fn entry(metadata: ExtensionMetadata) -> StaticRegistryEntry {
    StaticRegistryEntry {
        download_url: format!("https://example.com/{}.tar.gz", metadata.id),
        sha256: "0".repeat(64),
        metadata,
    }
}

fn query(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn timestamp(day: u32) -> String {
    format!("2026-01-{day:02}T00:00:00Z")
}

#[test]
fn browse_list_is_empty_only_when_provides_matches_nothing() {
    let entries = vec![
        entry(metadata("alpha", "Alpha", "1.0.0", &timestamp(1), &["themes"])),
        entry(metadata(
            "beta",
            "Beta",
            "1.0.0",
            &timestamp(2),
            &["languages"],
        )),
    ];

    // The UI always sends `provides`, empty when no category is selected. An
    // empty value has to mean "no filter", otherwise the "All" list is blank.
    let unfiltered = filter_static_registry(
        &entries,
        "/extensions",
        &query(&[("provides", "")]),
        ReleaseChannel::Stable,
    );
    assert_eq!(unfiltered.len(), 2);

    let filtered = filter_static_registry(
        &entries,
        "/extensions",
        &query(&[("provides", "themes")]),
        ReleaseChannel::Stable,
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id.as_ref(), "alpha");
}

#[test]
fn search_matches_id_name_and_description() {
    let mut extended = metadata("alpha", "Alpha", "1.0.0", &timestamp(1), &["themes"]);
    extended.manifest.description = Some("A calm color scheme".to_owned());
    let entries = vec![
        entry(extended),
        entry(metadata("beta", "Beta", "1.0.0", &timestamp(2), &["themes"])),
    ];

    let by_id = filter_static_registry(
        &entries,
        "/extensions",
        &query(&[("filter", "ALP")]),
        ReleaseChannel::Stable,
    );
    assert_eq!(by_id.len(), 1);
    assert_eq!(by_id[0].id.as_ref(), "alpha");

    let by_description = filter_static_registry(
        &entries,
        "/extensions",
        &query(&[("filter", "calm")]),
        ReleaseChannel::Stable,
    );
    assert_eq!(by_description.len(), 1);
    assert_eq!(by_description[0].id.as_ref(), "alpha");
}

#[test]
fn updates_without_ids_returns_nothing() {
    let entries = vec![entry(metadata(
        "alpha",
        "Alpha",
        "1.0.0",
        &timestamp(1),
        &["themes"],
    ))];

    // An empty id list means "nothing to update". Returning the whole catalog
    // here would upgrade extensions the user excluded from auto-update.
    let missing = filter_static_registry(
        &entries,
        "/extensions/updates",
        &query(&[]),
        ReleaseChannel::Stable,
    );
    assert!(missing.is_empty());

    let empty = filter_static_registry(
        &entries,
        "/extensions/updates",
        &query(&[("ids", "")]),
        ReleaseChannel::Stable,
    );
    assert!(empty.is_empty());
}

#[test]
fn updates_returns_only_the_named_ids() {
    let entries = vec![
        entry(metadata("alpha", "Alpha", "1.0.0", &timestamp(1), &["themes"])),
        entry(metadata("beta", "Beta", "1.0.0", &timestamp(2), &["themes"])),
    ];

    let updates = filter_static_registry(
        &entries,
        "/extensions/updates",
        &query(&[("ids", "beta")]),
        ReleaseChannel::Stable,
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].id.as_ref(), "beta");
}

#[test]
fn version_query_returns_every_version() {
    let entries = vec![
        entry(metadata("alpha", "Alpha", "1.0.0", &timestamp(1), &["themes"])),
        entry(metadata("alpha", "Alpha", "2.0.0", &timestamp(2), &["themes"])),
    ];

    let versions = filter_static_registry(
        &entries,
        "/extensions/alpha",
        &query(&[]),
        ReleaseChannel::Stable,
    );
    assert_eq!(versions.len(), 2);
}

#[test]
fn browse_list_keeps_the_newest_version_of_each_extension() {
    let entries = vec![
        entry(metadata("alpha", "Alpha", "1.0.0", &timestamp(1), &["themes"])),
        entry(metadata("alpha", "Alpha", "2.0.0", &timestamp(2), &["themes"])),
        entry(metadata("beta", "Beta", "1.0.0", &timestamp(3), &["themes"])),
    ];

    let browse = filter_static_registry(
        &entries,
        "/extensions",
        &query(&[]),
        ReleaseChannel::Stable,
    );
    assert_eq!(browse.len(), 2);
    let alpha = browse
        .iter()
        .find(|extension| extension.id.as_ref() == "alpha")
        .expect("alpha is listed");
    assert_eq!(alpha.manifest.version.as_ref(), "2.0.0");
}

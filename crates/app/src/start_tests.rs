use super::*;

/// The settings file names each kind plainly (`"kind": "Folder"`), so it reads — and a renamed variant shows.
#[test]
fn every_source_kind_serialises_with_its_kind_tag() {
    let s = |n: &str| n.to_string();
    for (source, kind) in [
        (
            Source::Folder {
                path: PathBuf::from("/p"),
            },
            "Folder",
        ),
        (Source::AllPhotos, "AllPhotos"),
        (
            Source::Smart {
                id: 1,
                name: s("a"),
            },
            "Smart",
        ),
        (
            Source::Periods {
                periods: vec![(2026, None, None)],
                label: s("2026"),
            },
            "Periods",
        ),
        (
            Source::Collection {
                id: 1,
                name: s("a"),
            },
            "Collection",
        ),
        (
            Source::Publish {
                id: 1,
                name: s("a"),
            },
            "Publish",
        ),
        (
            Source::Keyword {
                id: 1,
                name: s("a"),
            },
            "Keyword",
        ),
        (
            Source::Person {
                id: 1,
                name: s("a"),
            },
            "Person",
        ),
        (
            Source::ImmichPerson {
                id: s("u"),
                person: 1,
                name: s("a"),
            },
            "ImmichPerson",
        ),
        (
            Source::ImmichAlbum {
                id: s("u"),
                name: s("a"),
            },
            "ImmichAlbum",
        ),
        (
            Source::ImmichBucket {
                bucket: s("b"),
                label: s("a"),
            },
            "ImmichBucket",
        ),
    ] {
        let v = serde_json::to_value(&source).unwrap();
        assert_eq!(v["kind"], kind, "{source:?}");
        let back: Source = serde_json::from_value(v).unwrap();
        assert_eq!(back, source);
    }
}

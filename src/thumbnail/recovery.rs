type RecoveryItem = (String, Option<i64>, Option<i64>);

/// Use fresh source probes: a cached negative result can outlive a reconnect.
pub fn recovery_items(items: Vec<RecoveryItem>) -> (Vec<RecoveryItem>, usize) {
    let mut ready = Vec::new();
    let mut offline = 0;
    for item in items {
        let Ok(destination) = cache_path(&item.0, item.1, item.2) else {
            continue;
        };
        if destination.is_file() || known_decode_failure(&item.0, &destination) {
            continue;
        }
        if crate::source::file_available(&item.0) {
            ready.push(item);
        } else {
            offline += 1;
        }
    }
    (ready, offline)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use gio::prelude::*;

    struct Fixture {
        directory: PathBuf,
        item: RecoveryItem,
        destination: PathBuf,
    }

    impl Fixture {
        fn new(uri: bool) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = std::env::temp_dir().join(format!(
                "picasa-offline-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("photo.png");
            let reference = if uri {
                gio::File::for_path(&path).uri().to_string()
            } else {
                path.to_string_lossy().into_owned()
            };
            let destination = cache_path(&reference, Some(123), Some(456)).unwrap();
            Self {
                directory,
                item: (reference, Some(123), Some(456)),
                destination,
            }
        }

        fn reconnect(&self) {
            image::RgbImage::from_pixel(16, 12, image::Rgb([20, 90, 160]))
                .save(self.directory.join("photo.png"))
                .unwrap();
        }

        fn create(&self) -> Result<PathBuf> {
            create(&self.item.0, self.item.1, self.item.2)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
            let _ = fs::remove_file(&self.destination);
            let _ = fs::remove_file(self.destination.with_extension("failed"));
        }
    }

    #[test]
    fn offline_startup_recovers_after_reconnect_even_with_stale_uri_availability() {
        for uri in [false, true] {
            let fixture = Fixture::new(uri);
            assert!(!crate::source::cached_file_available(&fixture.item.0));
            assert_eq!(recovery_items(vec![fixture.item.clone()]), (vec![], 1));
            assert!(!fixture.destination.with_extension("failed").exists());
            fixture.reconnect();
            let (ready, offline) = recovery_items(vec![fixture.item.clone()]);
            assert_eq!(offline, 0);
            assert_eq!(ready.len(), 1);
            let results = create_many_cancellable(&ready, || false, |_| {});
            assert!(results[0].as_ref().unwrap().is_ok());
            assert!(fixture.destination.is_file());
            assert_eq!(recovery_items(vec![fixture.item.clone()]), (vec![], 0));
        }
    }

    #[test]
    fn disconnect_during_work_does_not_poison_the_cache() {
        let fixture = Fixture::new(false);
        assert!(fixture.create().is_err());
        assert!(!fixture.destination.with_extension("failed").exists());
        fixture.reconnect();
        assert!(fixture.create().unwrap().is_file());
    }

    #[test]
    fn legacy_offline_failure_marker_is_retried() {
        let fixture = Fixture::new(false);
        fs::write(
            fixture.destination.with_extension("failed"),
            b"thumbnail generation failed\n",
        )
        .unwrap();
        fixture.reconnect();
        assert_eq!(recovery_items(vec![fixture.item.clone()]).0.len(), 1);
        assert!(fixture.create().unwrap().is_file());
        assert!(!fixture.destination.with_extension("failed").exists());
    }

    #[test]
    fn corrupt_online_source_is_still_suppressed() {
        let fixture = Fixture::new(false);
        fs::write(fixture.directory.join("photo.png"), b"not an image").unwrap();
        assert!(fixture.create().is_err());
        assert!(known_decode_failure(&fixture.item.0, &fixture.destination));
        assert_eq!(recovery_items(vec![fixture.item.clone()]), (vec![], 0));
    }
}

#[cfg(test)]
mod viewer_presentation_tests {
    use super::*;

    fn request_key(name: &str) -> ViewerRequestKey {
        ViewerRequestKey {
            path: format!("/test/{name}.jpg"),
            mtime: 1,
            size_bytes: 1,
            rotation: 0,
            edit_recipe: String::new(),
            target_width: 800,
            target_height: 600,
        }
    }

    #[test]
    fn foreground_promotes_prefetch_without_losing_its_work() {
        let key = request_key("promote");
        let (prefetch_request, prefetch, first) = claim_viewer_request(&key, false);
        assert!(matches!(first, ViewerRequestClaim::New));
        let (foreground_request, foreground, joined) = claim_viewer_request(&key, true);
        assert!(Arc::ptr_eq(&prefetch_request, &foreground_request));
        assert!(matches!(joined, ViewerRequestClaim::PromotedPrefetch));

        // Replacing the old prefetch lease leaves the promoted foreground
        // lease alive, so the shared worker remains eligible to finish.
        prefetch.cancel();
        assert!(foreground_request.has_consumers());
        assert!(foreground_request.foreground.load(Ordering::Acquire));

        finish_viewer_request(&key, &foreground_request, Err(Arc::from("test complete")));
        foreground.release();
    }

    #[test]
    fn identical_foregrounds_join_one_inflight_request() {
        let key = request_key("foreground-join");
        let (first_request, first, created) = claim_viewer_request(&key, true);
        assert!(matches!(created, ViewerRequestClaim::New));
        let (second_request, second, joined) = claim_viewer_request(&key, true);
        assert!(Arc::ptr_eq(&first_request, &second_request));
        assert!(matches!(joined, ViewerRequestClaim::JoinedForeground));
        assert_eq!(first_request.consumers.load(Ordering::Acquire), 2);

        first.cancel();
        assert!(second_request.has_consumers());
        finish_viewer_request(&key, &second_request, Err(Arc::from("test complete")));
        second.release();
    }

    #[test]
    fn older_generation_is_not_current() {
        assert!(viewer_generation_current(9, 9));
        assert!(!viewer_generation_current(10, 9));
    }

    #[test]
    fn large_photo_preview_uses_final_fitted_size() {
        assert_eq!(
            fitted_picture_dimensions(6016, 4016, 320, 214, 1036, 794, 0.0),
            (1036, 692)
        );
    }

    #[test]
    fn oriented_preview_swaps_native_axes_before_fitting() {
        assert_eq!(
            fitted_picture_dimensions(6016, 4016, 214, 320, 1036, 794, 0.0),
            (530, 794)
        );
    }

    #[test]
    fn small_native_photo_is_not_upscaled() {
        assert_eq!(
            fitted_picture_dimensions(226, 320, 226, 320, 1036, 794, 0.0),
            (226, 320)
        );
    }

    #[test]
    fn metadata_unknown_preview_fills_the_viewer_while_decode_is_pending() {
        assert_eq!(
            fitted_picture_dimensions(0, 0, 240, 320, 1036, 794, 0.0),
            (596, 794)
        );
    }

    #[test]
    fn one_to_one_uses_decoded_raw_preview_dimensions() {
        assert_eq!(
            fitted_picture_dimensions(6016, 4016, 1620, 1080, 1036, 794, -1.0),
            (1620, 1080)
        );
    }

    #[test]
    fn one_to_one_uses_intrinsic_dimensions_when_metadata_is_unknown() {
        assert_eq!(
            fitted_picture_dimensions(0, 0, 3936, 2624, 1036, 794, -1.0),
            (3936, 2624)
        );
    }
}

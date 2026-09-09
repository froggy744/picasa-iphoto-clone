#[cfg(test)]
mod viewer_presentation_tests {
    use super::*;

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

pub fn dimensions(reference: &str, bytes: &[u8]) -> Result<(u32, u32)> {
    if is_raw(reference) {
        let local = crate::source::materialize(reference)?;
        let rawfile = rawler::rawsource::RawSource::new(&local)?;
        let decoder = rawler::get_decoder(&rawfile)?;
        // `dummy = true` reads the RAW geometry without unpacking the sensor
        // pixels. Prefer the recommended crop shown by photo applications.
        let raw = decoder.raw_image(
            &rawfile,
            &rawler::decoders::RawDecodeParams::default(),
            true,
        )?;
        let (width, height) = raw
            .crop_area
            .or(raw.active_area)
            .map(|area| (area.d.w, area.d.h))
            .unwrap_or((raw.width, raw.height));
        Ok((u32::try_from(width)?, u32::try_from(height)?))
    } else if is_jpeg(reference) {
        // Do not use image-rs' zune-jpeg dimension reader here. Some corrupt
        // JPEG APP segments can make that parser attempt an unchecked huge
        // allocation and abort the process instead of returning an error.
        let mut decompressor = Decompressor::new()
            .map_err(|error| anyhow::anyhow!("TurboJPEG initialization failed: {error}"))?;
        let header = decompressor
            .read_header(bytes)
            .map_err(|error| anyhow::anyhow!("invalid JPEG header: {error}"))?;
        Ok((header.width as u32, header.height as u32))
    } else if is_heif(reference) {
        let decoded = decode_heif(bytes)?;
        Ok((decoded.source_width, decoded.source_height))
    } else {
        Ok(ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()?
            .into_dimensions()?)
    }
}

fn decode_with_image(bytes: &[u8]) -> Result<DecodedThumbnailSource> {
    let source = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;
    let source_width = source.width();
    let source_height = source.height();
    let image = match source {
        DynamicImage::ImageRgb8(image) => image,
        image => image.to_rgb8(),
    };
    Ok(DecodedThumbnailSource {
        image,
        source_width,
        source_height,
        scale: "1/1",
    })
}

fn decode_jpeg_turbo(bytes: &[u8]) -> Result<DecodedThumbnailSource> {
    decode_jpeg_turbo_with_max(bytes, THUMBNAIL_SIZE)
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let mut decompressor = Decompressor::new()
        .map_err(|error| anyhow::anyhow!("TurboJPEG initialization failed: {error}"))?;
    let header = decompressor
        .read_header(bytes)
        .map_err(|error| anyhow::anyhow!("TurboJPEG header decode failed: {error}"))?;
    Ok((header.width as u32, header.height as u32))
}

fn decode_jpeg_turbo_with_target(
    bytes: &[u8],
    target_width: u32,
    target_height: u32,
) -> Result<DecodedThumbnailSource> {
    decode_jpeg_turbo_with_scale(bytes, |header| {
        native_scale_for_dimensions(header.width, header.height, target_width, target_height)
    })
}

fn decode_jpeg_turbo_with_max(bytes: &[u8], max_size: u32) -> Result<DecodedThumbnailSource> {
    decode_jpeg_turbo_with_scale(bytes, |header| {
        native_scale_for(header.width.max(header.height), max_size)
    })
}

fn decode_jpeg_turbo_with_scale(
    bytes: &[u8],
    select_scale: impl FnOnce(&turbojpeg::DecompressHeader) -> (ScalingFactor, &'static str),
) -> Result<DecodedThumbnailSource> {
    let mut decompressor = Decompressor::new()
        .map_err(|error| anyhow::anyhow!("TurboJPEG initialization failed: {error}"))?;
    let header = decompressor
        .read_header(bytes)
        .map_err(|error| anyhow::anyhow!("TurboJPEG header decode failed: {error}"))?;
    if header.is_lossless {
        return Err(anyhow::anyhow!(
            "lossless JPEG is not supported by scaled TurboJPEG decode"
        ));
    }

    let scale = select_scale(&header);
    decompressor
        .set_scaling_factor(scale.0)
        .map_err(|error| anyhow::anyhow!("TurboJPEG scale selection failed: {error}"))?;
    let scaled = header.scaled(scale.0);
    let mut pixels = vec![0u8; scaled.width * scaled.height * 3];
    let output = TurboImage {
        pixels: pixels.as_mut_slice(),
        width: scaled.width,
        pitch: scaled.width * 3,
        height: scaled.height,
        format: PixelFormat::RGB,
    };
    decompressor
        .decompress(bytes, output)
        .map_err(|error| anyhow::anyhow!("TurboJPEG pixel decode failed: {error}"))?;
    let image = image::RgbImage::from_raw(scaled.width as u32, scaled.height as u32, pixels)
        .context("TurboJPEG returned an invalid RGB buffer")?;
    Ok(DecodedThumbnailSource {
        image,
        source_width: header.width as u32,
        source_height: header.height as u32,
        scale: scale.1,
    })
}

fn native_scale_for_dimensions(
    source_width: usize,
    source_height: usize,
    target_width: u32,
    target_height: u32,
) -> (ScalingFactor, &'static str) {
    [
        (ScalingFactor::ONE_EIGHTH, "1/8"),
        (ScalingFactor::ONE_QUARTER, "1/4"),
        (ScalingFactor::ONE_HALF, "1/2"),
        (ScalingFactor::ONE, "1/1"),
    ]
    .into_iter()
    .find(|(factor, _)| {
        let scaled_width = factor.scale(source_width);
        let scaled_height = factor.scale(source_height);
        scaled_width >= target_width as usize && scaled_height >= target_height as usize
    })
    .unwrap_or((ScalingFactor::ONE, "1/1"))
}

fn native_scale_for(longest_dimension: usize, target: u32) -> (ScalingFactor, &'static str) {
    if longest_dimension / 8 >= target as usize {
        (ScalingFactor::ONE_EIGHTH, "1/8")
    } else if longest_dimension / 4 >= target as usize {
        (ScalingFactor::ONE_QUARTER, "1/4")
    } else if longest_dimension / 2 >= target as usize {
        (ScalingFactor::ONE_HALF, "1/2")
    } else {
        (ScalingFactor::ONE, "1/1")
    }
}


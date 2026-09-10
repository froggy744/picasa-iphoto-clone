/// Nikon Z cameras commonly put a 160x120 RGB thumbnail directly in the root
/// TIFF IFD rather than in a JPEG thumbnail directory. Reading its single
/// strip avoids decoding any large preview or RAW sensor data.
fn nef_uncompressed_thumbnail(path: &std::path::Path) -> Result<Option<DecodedThumbnailSource>> {
    let mut file = fs::File::open(path)?;
    let mut header = [0; 8];
    file.read_exact(&mut header)?;
    let little_endian = match &header[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Ok(None),
    };
    if tiff_u16(&header[2..4], little_endian) != 42 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(u64::from(tiff_u32(
        &header[4..8],
        little_endian,
    ))))?;
    let count = read_tiff_u16(&mut file, little_endian)? as usize;
    if count > 1024 {
        return Ok(None);
    }
    let (mut width, mut height, mut compression, mut samples, mut offset, mut length) =
        (None, None, None, None, None, None);
    for _ in 0..count {
        let mut entry = [0; 12];
        file.read_exact(&mut entry)?;
        let tag = tiff_u16(&entry[..2], little_endian);
        let field_type = tiff_u16(&entry[2..4], little_endian);
        let item_count = tiff_u32(&entry[4..8], little_endian);
        if item_count != 1 {
            continue;
        }
        let value = match field_type {
            3 => u32::from(tiff_u16(&entry[8..10], little_endian)),
            4 => tiff_u32(&entry[8..12], little_endian),
            _ => continue,
        };
        match tag {
            0x0100 => width = Some(value),
            0x0101 => height = Some(value),
            0x0103 => compression = Some(value),
            0x0111 => offset = Some(value),
            0x0115 => samples = Some(value),
            0x0117 => length = Some(value),
            _ => {}
        }
    }
    let (Some(width), Some(height), Some(1), Some(3), Some(offset), Some(length)) =
        (width, height, compression, samples, offset, length)
    else {
        return Ok(None);
    };
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3));
    if width == 0
        || height == 0
        || expected != Some(length)
        || u64::from(offset) + u64::from(length) > file.metadata()?.len()
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut pixels = vec![0; length as usize];
    file.read_exact(&mut pixels)?;
    let image = image::RgbImage::from_raw(width, height, pixels)
        .context("invalid NEF embedded RGB thumbnail")?;
    Ok(Some(DecodedThumbnailSource {
        image,
        source_width: width,
        source_height: height,
        scale: "embedded TIFF thumbnail",
    }))
}

fn nef_embedded_thumbnail(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    read_nef_jpeg(path, false)
}

fn nef_embedded_preview(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    read_nef_jpeg(path, true)
}

fn read_nef_jpeg(path: &std::path::Path, largest: bool) -> Result<Option<Vec<u8>>> {
    let mut file = fs::File::open(path)?;
    let file_size = file.metadata()?.len();
    let Some((offset, length)) = nef_jpeg_stream(&mut file, largest)? else {
        return Ok(None);
    };
    // A malformed tag must never request an unbounded allocation or seek.
    if length == 0
        || length > 100 * 1024 * 1024
        || u64::from(offset) + u64::from(length) > file_size
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    Ok((bytes.starts_with(&[0xff, 0xd8])).then_some(bytes))
}

/// Locate JPEGInterchangeFormat streams in classic TIFF IFDs, including the
/// Nikon SubIFDs that kamadak-exif deliberately does not expose as IFD1.
fn nef_jpeg_stream(file: &mut fs::File, largest: bool) -> Result<Option<(u32, u32)>> {
    let mut header = [0; 8];
    file.read_exact(&mut header)?;
    let little_endian = match &header[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Ok(None),
    };
    if tiff_u16(&header[2..4], little_endian) != 42 {
        return Ok(None);
    }
    let mut pending = vec![tiff_u32(&header[4..8], little_endian)];
    let mut visited = std::collections::HashSet::new();
    let mut candidates = Vec::new();

    while let Some(ifd_offset) = pending.pop() {
        if ifd_offset == 0 || !visited.insert(ifd_offset) || visited.len() > 256 {
            continue;
        }
        file.seek(SeekFrom::Start(u64::from(ifd_offset)))?;
        let count = read_tiff_u16(file, little_endian)? as usize;
        if count > 1024 {
            continue;
        }
        let mut jpeg_offset = None;
        let mut jpeg_length = None;
        for _ in 0..count {
            let mut entry = [0; 12];
            file.read_exact(&mut entry)?;
            let tag = tiff_u16(&entry[..2], little_endian);
            let field_type = tiff_u16(&entry[2..4], little_endian);
            let item_count = tiff_u32(&entry[4..8], little_endian);
            let value = tiff_u32(&entry[8..12], little_endian);
            match tag {
                0x0201 if item_count == 1 && matches!(field_type, 3 | 4) => {
                    jpeg_offset = Some(value)
                }
                0x0202 if item_count == 1 && matches!(field_type, 3 | 4) => {
                    jpeg_length = Some(value)
                }
                // SubIFDs may point to several preview/thumbnail directories.
                0x014a if field_type == 4 && item_count > 0 && item_count <= 64 => {
                    if item_count == 1 {
                        pending.push(value);
                    } else {
                        let return_position = file.stream_position()?;
                        file.seek(SeekFrom::Start(u64::from(value)))?;
                        for _ in 0..item_count {
                            pending.push(read_tiff_u32(file, little_endian)?);
                        }
                        file.seek(SeekFrom::Start(return_position))?;
                    }
                }
                // Nikon often stores a preview IFD through this private tag.
                0x8769 | 0x014a if field_type == 4 && item_count == 1 => pending.push(value),
                _ => {}
            }
        }
        if let (Some(offset), Some(length)) = (jpeg_offset, jpeg_length) {
            candidates.push((offset, length));
        }
        pending.push(read_tiff_u32(file, little_endian)?);
    }
    // The smallest JPEG stream is Nikon's fast thumbnail. Larger streams are
    // full camera previews and are selected by the lightbox.
    let candidates = candidates.into_iter().filter(|(_, length)| *length > 0);
    Ok(if largest {
        candidates.max_by_key(|(_, length)| *length)
    } else {
        candidates.min_by_key(|(_, length)| *length)
    })
}

fn read_tiff_u16(file: &mut fs::File, little_endian: bool) -> Result<u16> {
    let mut bytes = [0; 2];
    file.read_exact(&mut bytes)?;
    Ok(tiff_u16(&bytes, little_endian))
}

fn read_tiff_u32(file: &mut fs::File, little_endian: bool) -> Result<u32> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)?;
    Ok(tiff_u32(&bytes, little_endian))
}

fn tiff_u16(bytes: &[u8], little_endian: bool) -> u16 {
    if little_endian {
        u16::from_le_bytes([bytes[0], bytes[1]])
    } else {
        u16::from_be_bytes([bytes[0], bytes[1]])
    }
}

fn tiff_u32(bytes: &[u8], little_endian: bool) -> u32 {
    if little_endian {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}


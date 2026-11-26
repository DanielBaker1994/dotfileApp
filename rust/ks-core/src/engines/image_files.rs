//! Is this file really a picture? Decided by its first bytes (magic
//! numbers), never by its extension: a `.png` that is really text, or a
//! `.pdf` / `.psd` / `.mov`, is not offered to the screenshot editor, and a
//! photo with a wrong or missing extension still is.
//!
//! Accepted: PNG, JPEG, GIF, TIFF (incl. camera DNG), BMP, WebP and the
//! ISO-BMFF stills (HEIC / HEIF / AVIF — what an iPhone saves).
//! Pure: no AppKit, so it is unit-tested and portable. The decode step
//! (`views::screenshot::decode_image_file`) is the second gate.

use std::io::Read;

/// Bytes `sniff_image` needs to decide.
pub const SNIFF_LEN: usize = 32;

/// The image format `head` (a file's first bytes) starts with, or `None`.
pub fn sniff_image(head: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| head.len() >= m.len() && &head[..m.len()] == m;
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("gif");
    }
    if starts(b"II*\0") || starts(b"MM\0*") {
        return Some("tiff");
    }
    // "BM" + a sane DIB header size (12, 40, 52, 56, 64, 108, 124): a bare
    // "BM" prefix alone is too common in text files.
    if starts(b"BM") && head.len() >= 18 {
        let dib = u32::from_le_bytes([head[14], head[15], head[16], head[17]]);
        if matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124) {
            return Some("bmp");
        }
    }
    if head.len() >= 12 && starts(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some("webp");
    }
    // ISO-BMFF: [size:4]["ftyp"][major brand:4][minor:4][compatible brands…].
    // Only still-image brands — an MP4 / MOV also starts with `ftyp`.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        const STILL: [&[u8; 4]; 8] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"mif1", b"msf1", b"avif"];
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let end = size.clamp(12, head.len());
        let major = &head[8..12];
        let is_still = |b: &[u8]| STILL.iter().any(|s| &s[..] == b);
        let video_major = [&b"qt  "[..], b"isom", b"mp41", b"mp42", b"M4V ", b"3gp4"].contains(&major);
        let still_compat = head[16.min(end)..end].chunks_exact(4).any(is_still);
        if is_still(major) || (!video_major && still_compat) {
            return Some(if major == b"avif" { "avif" } else { "heic" });
        }
    }
    None
}

/// `path` is a regular file whose bytes start like a picture.
pub fn is_image_file(path: &str) -> bool {
    let p = std::path::Path::new(path);
    if !p.is_file() {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut head = [0u8; SNIFF_LEN];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return false,
        }
    }
    sniff_image(&head[..n]).is_some()
}

/// Every path is a picture (and there is at least one): the selection can go
/// to the screenshot editor. One non-image hides the menu item.
pub fn all_images(paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| is_image_file(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compat.len();
        let mut v = (size as u32).to_be_bytes().to_vec();
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(major);
        v.extend_from_slice(&[0, 0, 0, 0]);
        for c in compat {
            v.extend_from_slice(*c);
        }
        v
    }

    #[test]
    fn known_formats_are_pictures() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 0]), Some("jpeg"));
        assert_eq!(sniff_image(b"GIF89a\x01\0"), Some("gif"));
        assert_eq!(sniff_image(b"II*\0\x08\0\0\0"), Some("tiff"));
        assert_eq!(sniff_image(b"MM\0*\0\0\0\x08"), Some("tiff"));
        assert_eq!(sniff_image(b"RIFF\x24\0\0\0WEBPVP8 "), Some("webp"));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        assert_eq!(sniff_image(&bmp), Some("bmp"));
        assert_eq!(sniff_image(&ftyp(b"heic", &[b"mif1", b"heic"])), Some("heic"));
        assert_eq!(sniff_image(&ftyp(b"mif1", &[b"heic"])), Some("heic"));
        assert_eq!(sniff_image(&ftyp(b"avif", &[b"mif1"])), Some("avif"));
    }

    #[test]
    fn lookalikes_are_not_pictures() {
        assert_eq!(sniff_image(b"%PDF-1.7\n"), None, "pdf");
        assert_eq!(sniff_image(b"8BPS\0\x01"), None, "psd");
        assert_eq!(sniff_image(b"PK\x03\x04"), None, "zip / docx");
        assert_eq!(sniff_image(b"BMW is a car brand, not a bitmap"), None, "text starting BM");
        assert_eq!(sniff_image(b"RIFF\x24\0\0\0WAVEfmt "), None, "wav");
        assert_eq!(sniff_image(&ftyp(b"qt  ", &[b"qt  "])), None, "mov");
        assert_eq!(sniff_image(&ftyp(b"isom", &[b"isom", b"mp41"])), None, "mp4");
        assert_eq!(sniff_image(&ftyp(b"mp42", &[b"mp42", b"isom"])), None, "mp4");
        assert_eq!(sniff_image(b"\x89PN"), None, "truncated");
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn files_are_judged_by_bytes_not_extension() {
        let dir = std::env::temp_dir().join(format!("ks-imgsniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.to_string_lossy().into_owned()
        };
        let fake = w("fake.png", b"hello, I am text");
        let real = w("photo.dat", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F']);
        let png = w("a.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        assert!(!is_image_file(&fake), "a .png full of text is not a picture");
        assert!(is_image_file(&real), "a JPEG without the extension is");
        assert!(!is_image_file(&dir.to_string_lossy()), "a folder is not");
        assert!(!is_image_file("/nonexistent/x.png"));
        assert!(all_images(&[real.clone(), png.clone()]));
        assert!(!all_images(&[real, fake]), "one non-image hides the item");
        assert!(!all_images(&[]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

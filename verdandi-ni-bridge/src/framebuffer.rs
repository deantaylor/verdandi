#[derive(PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u16,
    pub height: u16,
    pub rgb: Vec<u8>,
}

pub fn render(source: &CapturedFrame, width: u16, height: u16) -> Option<Vec<u8>> {
    let (sw, sh) = (usize::from(source.width), usize::from(source.height));
    let (tw, th) = (usize::from(width), usize::from(height));
    let source_len = sw.checked_mul(sh)?.checked_mul(3)?;
    if sw == 0 || sh == 0 || tw == 0 || th == 0 || source.rgb.len() != source_len {
        return None;
    }
    let scale = (tw / sw).min(th / sh);
    let (w, h) = if scale > 0 {
        (sw * scale, sh * scale)
    } else if tw * sh <= th * sw {
        (tw, (tw * sh / sw).max(1))
    } else {
        ((th * sw / sh).max(1), th)
    };
    let (left, top) = ((tw - w) / 2, (th - h) / 2);
    let mut out = vec![0; tw.checked_mul(th)?.checked_mul(4)?];
    for alpha in out.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    for y in 0..h {
        let sy = y * sh / h;
        for x in 0..w {
            let sx = x * sw / w;
            let rgb = (sy * sw + sx) * 3;
            let rgba = ((top + y) * tw + left + x) * 4;
            out[rgba..rgba + 3].copy_from_slice(&source.rgb[rgb..rgb + 3]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_centres_the_norns_frame_at_three_times() {
        let source = CapturedFrame {
            width: 128,
            height: 64,
            rgb: vec![85; 128 * 64 * 3],
        };
        let out = render(&source, 480, 272).unwrap();
        let pixel = |x: usize, y: usize| &out[(y * 480 + x) * 4..][..4];
        assert_eq!(pixel(48, 40), [85, 85, 85, 255]);
        assert_eq!(pixel(431, 231), [85, 85, 85, 255]);
        assert_eq!(pixel(47, 40), [0, 0, 0, 255]);
        assert_eq!(pixel(432, 231), [0, 0, 0, 255]);
        assert_eq!(pixel(48, 39), [0, 0, 0, 255]);
        assert_eq!(pixel(48, 232), [0, 0, 0, 255]);
    }

    #[test]
    fn fits_smaller_screens_and_preserves_rgb_and_alpha() {
        let source = CapturedFrame {
            width: 4,
            height: 2,
            rgb: [15, 80, 112].repeat(8),
        };
        let out = render(&source, 2, 3).unwrap();
        assert_eq!(&out[8..16], [15, 80, 112, 255, 15, 80, 112, 255]);
        assert_eq!(&out[..8], [0, 0, 0, 255, 0, 0, 0, 255]);
        assert!(render(&source, 0, 3).is_none());
        assert!(render(
            &CapturedFrame {
                rgb: vec![],
                ..source
            },
            4,
            2
        )
        .is_none());
    }
}

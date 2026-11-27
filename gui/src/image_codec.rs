#[cfg(windows)]
mod platform {
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::mem::{size_of, zeroed, ManuallyDrop};
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::{HGLOBAL, HWND, RECT, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    GetDC, GetObjectW, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HALFTONE, HBITMAP,
    HGDIOBJ, HPALETTE, SRCCOPY,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_ContainerFormatPng,
    GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA, IWICBitmap, IWICBitmapSource,
    IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapEncoderNoCache,
    WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeMedianCut, WICBitmapUseAlpha,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::{
    CreateStreamOnHGlobal, IPropertyBag2, PROPBAG2,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IStream, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_R4};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

pub struct ComScope {
    should_uninit: bool,
}

impl ComScope {
    pub fn ensure_mta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_OK (0) or S_FALSE (1) means initialized on this thread; RPC_E_CHANGED_MODE means already initialized as STA.
        let should_uninit = hr != RPC_E_CHANGED_MODE && hr.is_ok();
        Self { should_uninit }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe { CoUninitialize() };
        }
    }
}

pub struct OwnedBitmap(pub HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0 .0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0 .0));
            }
        }
    }
}

pub fn grab_window_bitmap(window: HWND) -> Result<(OwnedBitmap, i32, i32), String> {
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(window, &mut bounds) }
        .map_err(|_| "対象ウィンドウの位置を取得できません".to_string())?;
    let w = bounds.right - bounds.left;
    let h = bounds.bottom - bounds.top;
    if w <= 0 || h <= 0 {
        return Err("対象ウィンドウの位置を取得できません".into());
    }
    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.is_invalid() {
        return Err("画面DCを取得できません".into());
    }
    let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    let bmp = if !mem_dc.is_invalid() {
        unsafe { CreateCompatibleBitmap(screen_dc, w, h) }
    } else {
        HBITMAP::default()
    };
    if mem_dc.is_invalid() || bmp.is_invalid() {
        if !bmp.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
        }
        if !mem_dc.is_invalid() {
            unsafe {
                let _ = DeleteDC(mem_dc);
            }
        }
        unsafe {
            let _ = ReleaseDC(None, screen_dc);
        }
        return Err("スクリーンショット用ビットマップを作成できません".into());
    }
    let old = unsafe { SelectObject(mem_dc, HGDIOBJ(bmp.0)) };
    let blt = unsafe {
        BitBlt(
            mem_dc,
            0,
            0,
            w,
            h,
            Some(screen_dc),
            bounds.left,
            bounds.top,
            SRCCOPY | CAPTUREBLT,
        )
    };
    unsafe {
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);
    }
    if blt.is_err() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        return Err("画面をキャプチャできません".into());
    }
    Ok((OwnedBitmap(bmp), w, h))
}

pub fn extract_luma_32x18(hbitmap: HBITMAP) -> Vec<u8> {
    if hbitmap.is_invalid() || hbitmap.0.is_null() {
        return Vec::new();
    }
    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(hbitmap.0),
            size_of::<BITMAP>() as i32,
            Some((&mut bm as *mut BITMAP).cast()),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return Vec::new();
    }
    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.is_invalid() {
        return Vec::new();
    }
    let src_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    let dst_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: 32,
        biHeight: -18,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut out = Vec::new();
    if !src_dc.is_invalid() && !dst_dc.is_invalid() {
        if let Ok(dib) =
            unsafe { CreateDIBSection(Some(dst_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
        {
            if !dib.is_invalid() && !bits.is_null() {
                let old_src = unsafe { SelectObject(src_dc, HGDIOBJ(hbitmap.0)) };
                let old_dst = unsafe { SelectObject(dst_dc, HGDIOBJ(dib.0)) };
                unsafe {
                    let _ = SetStretchBltMode(dst_dc, HALFTONE);
                    let _ = SetBrushOrgEx(dst_dc, 0, 0, None);
                }
                let ok = unsafe {
                    StretchBlt(
                        dst_dc,
                        0,
                        0,
                        32,
                        18,
                        Some(src_dc),
                        0,
                        0,
                        bm.bmWidth,
                        bm.bmHeight,
                        SRCCOPY,
                    )
                };
                if ok.as_bool() {
                    let px = unsafe { std::slice::from_raw_parts(bits as *const u8, 32 * 18 * 4) };
                    out.resize(32 * 18, 0);
                    for i in 0..(32 * 18) {
                        let b = px[i * 4] as u32;
                        let g = px[i * 4 + 1] as u32;
                        let r = px[i * 4 + 2] as u32;
                        out[i] = ((77 * r + 150 * g + 29 * b) >> 8) as u8;
                    }
                }
                unsafe {
                    let _ = SelectObject(dst_dc, old_dst);
                    let _ = SelectObject(src_dc, old_src);
                    let _ = DeleteObject(HGDIOBJ(dib.0));
                }
            }
        }
    }
    unsafe {
        if !dst_dc.is_invalid() {
            let _ = DeleteDC(dst_dc);
        }
        if !src_dc.is_invalid() {
            let _ = DeleteDC(src_dc);
        }
        let _ = ReleaseDC(None, screen_dc);
    }
    out
}

pub fn compute_luma_motion_score(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| x.abs_diff(y) as u64)
        .sum();
    (sum as f64) / ((a.len() as f64) * 255.0)
}

fn create_wic_factory() -> Result<IWICImagingFactory, String> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("WIC initialization failed: {}", e))
}

fn read_istream_bytes(stream: &IStream) -> Result<Vec<u8>, String> {
    let mut stat = STATSTG::default();
    unsafe { stream.Stat(&mut stat, STATFLAG_NONAME) }
        .map_err(|e| format!("Stream stat failed: {}", e))?;
    if stat.cbSize == 0 || stat.cbSize > 67_108_864 {
        return Err("Encoded image size out of range".into());
    }
    let total = stat.cbSize as usize;
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }
        .map_err(|e| format!("Stream seek failed: {}", e))?;
    let mut buf = vec![0u8; total];
    let mut read = 0u32;
    let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), total as u32, Some(&mut read)) };
    if hr.is_err() || read as usize != total {
        return Err("Stream read failed".into());
    }
    Ok(buf)
}

fn write_bytes_to_stream(bytes: &[u8]) -> Result<IStream, String> {
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }
        .map_err(|e| format!("CreateStreamOnHGlobal failed: {}", e))?;
    let mut written = 0u32;
    let hr = unsafe { stream.Write(bytes.as_ptr().cast(), bytes.len() as u32, Some(&mut written)) };
    if hr.is_err() || written as usize != bytes.len() {
        return Err("Stream write failed".into());
    }
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }
        .map_err(|e| format!("Stream seek failed: {}", e))?;
    Ok(stream)
}

fn scale_and_convert(
    factory: &IWICImagingFactory,
    src: &IWICBitmapSource,
    max_dim: u32,
    target_fmt: windows::core::GUID,
) -> Result<IWICBitmapSource, String> {
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { src.GetSize(&mut w, &mut h) }.map_err(|e| e.to_string())?;
    if w == 0 || h == 0 {
        return Err("Zero-sized image".into());
    }
    let longest = w.max(h);
    let mut current: IWICBitmapSource = src.clone();
    if max_dim > 0 && longest > max_dim {
        let scale = (max_dim as f64) / (longest as f64);
        let dst_w = ((w as f64 * scale) as u32).max(1);
        let dst_h = ((h as f64 * scale) as u32).max(1);
        let scaler = unsafe { factory.CreateBitmapScaler() }.map_err(|e| e.to_string())?;
        unsafe {
            scaler.Initialize(
                &current,
                dst_w,
                dst_h,
                WICBitmapInterpolationModeHighQualityCubic,
            )
        }
        .map_err(|e| e.to_string())?;
        current = scaler.cast().map_err(|e| e.to_string())?;
    }
    let cur_fmt = unsafe { current.GetPixelFormat() }.map_err(|e| e.to_string())?;
    if cur_fmt != target_fmt {
        let conv = unsafe { factory.CreateFormatConverter() }.map_err(|e| e.to_string())?;
        unsafe {
            conv.Initialize(
                &current,
                &target_fmt,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )
        }
        .map_err(|e| e.to_string())?;
        current = conv.cast().map_err(|e| e.to_string())?;
    }
    Ok(current)
}

fn encode_source(
    factory: &IWICImagingFactory,
    src: &IWICBitmapSource,
    max_dim: u32,
    is_jpeg: bool,
    quality: f32,
) -> Result<Vec<u8>, String> {
    let target_fmt = if is_jpeg {
        GUID_WICPixelFormat24bppBGR
    } else {
        GUID_WICPixelFormat32bppBGRA
    };
    let container = if is_jpeg {
        GUID_ContainerFormatJpeg
    } else {
        GUID_ContainerFormatPng
    };
    let prepared = scale_and_convert(factory, src, max_dim, target_fmt)?;
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { prepared.GetSize(&mut w, &mut h) }.map_err(|e| e.to_string())?;

    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }
        .map_err(|e| e.to_string())?;
    let encoder = unsafe { factory.CreateEncoder(&container, std::ptr::null()) }
        .map_err(|e| e.to_string())?;
    unsafe { encoder.Initialize(&stream, WICBitmapEncoderNoCache) }.map_err(|e| e.to_string())?;

    let mut frame_opt = None;
    let mut bag_opt: Option<IPropertyBag2> = None;
    unsafe { encoder.CreateNewFrame(&mut frame_opt, &mut bag_opt) }.map_err(|e| e.to_string())?;
    let frame = frame_opt.ok_or_else(|| "Missing WIC frame".to_string())?;

    if is_jpeg {
        if let Some(ref bag) = bag_opt {
            let key_w: Vec<u16> = "ImageQuality\0".encode_utf16().collect();
            let mut pb2: PROPBAG2 = unsafe { zeroed() };
            pb2.pstrName = PWSTR(key_w.as_ptr() as *mut u16);
            let var = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_R4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 { fltVal: quality },
                    }),
                },
            };
            unsafe { bag.Write(1, &pb2, &var) }.map_err(|e| e.to_string())?;
        }
    }
    unsafe { frame.Initialize(bag_opt.as_ref()) }.map_err(|e| e.to_string())?;
    unsafe { frame.SetSize(w, h) }.map_err(|e| e.to_string())?;
    let mut fmt = target_fmt;
    unsafe { frame.SetPixelFormat(&mut fmt) }.map_err(|e| e.to_string())?;
    unsafe { frame.WriteSource(&prepared, std::ptr::null()) }.map_err(|e| e.to_string())?;
    unsafe { frame.Commit() }.map_err(|e| e.to_string())?;
    unsafe { encoder.Commit() }.map_err(|e| e.to_string())?;
    read_istream_bytes(&stream)
}

fn compress_wic_source_to_budget(
    factory: &IWICImagingFactory,
    src: &IWICBitmapSource,
    max_bytes: usize,
    try_original_png: bool,
) -> Option<(Vec<u8>, &'static str)> {
    let (mut w, mut h) = (0u32, 0u32);
    if unsafe { src.GetSize(&mut w, &mut h) }.is_err() || w == 0 || h == 0 || max_bytes == 0 {
        return None;
    }
    let longest = w.max(h);
    for dim in [0u32, 1920, 1440, 1280] {
        if (dim == 0 && !try_original_png) || (dim > 0 && longest <= dim) {
            continue;
        }
        if let Ok(bytes) = encode_source(factory, src, dim, false, 1.0) {
            if bytes.len() <= max_bytes {
                return Some((bytes, ".png"));
            }
        }
    }
    for q in [0.85f32, 0.72, 0.55] {
        for dim in [0u32, 1920, 1440, 1280, 1024, 768, 512] {
            if (dim == 0 && longest > 1920) || (dim > 0 && longest <= dim) {
                continue;
            }
            if let Ok(bytes) = encode_source(factory, src, dim, true, q) {
                if bytes.len() <= max_bytes {
                    return Some((bytes, ".jpg"));
                }
            }
        }
    }
    for dim in [1024u32, 768, 512, 384] {
        if longest > dim {
            if let Ok(bytes) = encode_source(factory, src, dim, false, 1.0) {
                if bytes.len() <= max_bytes {
                    return Some((bytes, ".png"));
                }
            }
        }
    }
    None
}

pub fn encode_hbitmap_jpeg(
    hbitmap: HBITMAP,
    max_bytes: usize,
    dims: [u32; 2],
    quals: [f32; 2],
) -> Option<Vec<u8>> {
    if hbitmap.is_invalid() || hbitmap.0.is_null() {
        return None;
    }
    let _com = ComScope::ensure_mta();
    let factory = create_wic_factory().ok()?;
    let wic_bmp: IWICBitmap = unsafe {
        factory.CreateBitmapFromHBITMAP(hbitmap, HPALETTE::default(), WICBitmapUseAlpha)
    }
    .ok()?;
    let src: IWICBitmapSource = wic_bmp.cast().ok()?;
    for pass in 0..2 {
        if let Ok(bytes) = encode_source(&factory, &src, dims[pass], true, quals[pass]) {
            if !bytes.is_empty() && (bytes.len() <= max_bytes || pass == 1) {
                return Some(bytes);
            }
        }
    }
    None
}

pub fn encode_hbitmap_to_budget(
    hbitmap: HBITMAP,
    max_bytes: usize,
) -> Option<(Vec<u8>, &'static str)> {
    if hbitmap.is_invalid() || hbitmap.0.is_null() {
        return None;
    }
    let _com = ComScope::ensure_mta();
    let factory = create_wic_factory().ok()?;
    let wic_bmp: IWICBitmap = unsafe {
        factory.CreateBitmapFromHBITMAP(hbitmap, HPALETTE::default(), WICBitmapUseAlpha)
    }
    .ok()?;
    let src: IWICBitmapSource = wic_bmp.cast().ok()?;
    if let Ok(raw_png) = encode_source(&factory, &src, 0, false, 1.0) {
        if raw_png.len() <= max_bytes {
            return Some((raw_png, ".png"));
        }
    }
    compress_wic_source_to_budget(&factory, &src, max_bytes, false)
}

pub fn compress_image_bytes_to_budget(
    raw_bytes: &[u8],
    max_bytes: usize,
) -> Option<(Vec<u8>, &'static str)> {
    if raw_bytes.is_empty() || max_bytes == 0 {
        return None;
    }
    let _com = ComScope::ensure_mta();
    let factory = create_wic_factory().ok()?;
    let stream = write_bytes_to_stream(raw_bytes).ok()?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
    }
    .ok()?;
    let frame = unsafe { decoder.GetFrame(0) }.ok()?;
    let src: IWICBitmapSource = frame.cast().ok()?;
    compress_wic_source_to_budget(&factory, &src, max_bytes, true)
}

pub fn make_thumbnail_data_url(raw_bytes: &[u8], max_dim: u32) -> Option<String> {
    if raw_bytes.is_empty() {
        return None;
    }
    let _com = ComScope::ensure_mta();
    let factory = create_wic_factory().ok()?;
    let stream = write_bytes_to_stream(raw_bytes).ok()?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
    }
    .ok()?;
    let frame = unsafe { decoder.GetFrame(0) }.ok()?;
    let src: IWICBitmapSource = frame.cast().ok()?;
    let png = encode_source(&factory, &src, max_dim, false, 1.0).ok()?;
    Some(format!("data:image/png;base64,{}", B64.encode(&png)))
}

pub fn dib_bytes_to_png(dib_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if dib_bytes.len() < size_of::<BITMAPINFOHEADER>() {
        return Err("Invalid clipboard DIB".into());
    }
    let hdr = unsafe { std::ptr::read_unaligned(dib_bytes.as_ptr().cast::<BITMAPINFOHEADER>()) };
    let w = hdr.biWidth;
    let h_raw = hdr.biHeight;
    let h = h_raw.abs();
    if w <= 0 || h <= 0 || w > 16384 || h > 16384 {
        return Err("Unsupported clipboard image dimensions".into());
    }
    let mut bmp_file = Vec::with_capacity(14 + dib_bytes.len());
    let color_cnt = if hdr.biBitCount <= 8 {
        if hdr.biClrUsed > 0 {
            hdr.biClrUsed
        } else {
            1u32 << hdr.biBitCount
        }
    } else if hdr.biCompression == 3 && hdr.biSize == size_of::<BITMAPINFOHEADER>() as u32 {
        3
    } else {
        0
    };
    let offset = 14u32 + hdr.biSize + color_cnt * 4;
    let total_size = (14 + dib_bytes.len()) as u32;
    bmp_file.extend_from_slice(b"BM");
    bmp_file.extend_from_slice(&total_size.to_le_bytes());
    bmp_file.extend_from_slice(&0u32.to_le_bytes());
    bmp_file.extend_from_slice(&offset.to_le_bytes());
    bmp_file.extend_from_slice(dib_bytes);

    let _com = ComScope::ensure_mta();
    let factory = create_wic_factory()?;
    let stream = write_bytes_to_stream(&bmp_file)?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
    }
    .map_err(|e| format!("Decode clipboard DIB failed: {}", e))?;
    let frame = unsafe { decoder.GetFrame(0) }.map_err(|e| e.to_string())?;
    let src: IWICBitmapSource = frame.cast().map_err(|e| e.to_string())?;
    encode_source(&factory, &src, 0, false, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{FillRect, GetStockObject, DKGRAY_BRUSH, HBRUSH, WHITE_BRUSH};

    #[test]
    fn self_test_bitmap_jpeg_and_luma() {
        unsafe {
            let screen_dc = GetDC(None);
            assert!(!screen_dc.is_invalid());
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            assert!(!mem_dc.is_invalid());
            let bmp = CreateCompatibleBitmap(screen_dc, 64, 64);
            let bmp_white = CreateCompatibleBitmap(screen_dc, 64, 64);
            assert!(!bmp.is_invalid() && !bmp_white.is_invalid());

            let r = RECT {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            };
            let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));
            FillRect(mem_dc, &r, HBRUSH(GetStockObject(DKGRAY_BRUSH).0));
            SelectObject(mem_dc, HGDIOBJ(bmp_white.0));
            FillRect(mem_dc, &r, HBRUSH(GetStockObject(WHITE_BRUSH).0));
            SelectObject(mem_dc, old);
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);

            let (encoded, ext) = encode_hbitmap_to_budget(bmp, 900_000).expect("encode to budget");
            assert!(!encoded.is_empty());
            assert!(ext == ".png" || ext == ".jpg");

            let jpg = encode_hbitmap_jpeg(bmp, 90_000, [960, 768], [0.75, 0.60]).expect("jpeg");
            assert!(jpg.starts_with(&[0xFF, 0xD8]));

            let luma_dark = extract_luma_32x18(bmp);
            let luma_white = extract_luma_32x18(bmp_white);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteObject(HGDIOBJ(bmp_white.0));

            assert_eq!(luma_dark.len(), 576);
            assert_eq!(luma_white.len(), 576);
            assert_eq!(compute_luma_motion_score(&luma_dark, &luma_dark), 0.0);
            assert!(compute_luma_motion_score(&luma_dark, &luma_white) > 0.015);
            assert_eq!(compute_luma_motion_score(&[], &luma_dark), 0.0);
        }
    }
}
}

#[cfg(windows)]
pub use platform::*;

#[cfg(not(windows))]
mod platform {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    use image::{DynamicImage, GenericImageView, ImageFormat};
    use std::io::Cursor;

    fn scale_image(img: &DynamicImage, max_dim: u32) -> DynamicImage {
        let (w, h) = img.dimensions();
        let longest = w.max(h);
        if max_dim > 0 && longest > max_dim {
            let scale = (max_dim as f64) / (longest as f64);
            let dst_w = ((w as f64 * scale) as u32).max(1);
            let dst_h = ((h as f64 * scale) as u32).max(1);
            img.resize(dst_w, dst_h, FilterType::Lanczos3)
        } else {
            img.clone()
        }
    }

    fn encode_png(img: &DynamicImage) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .ok()?;
        Some(out)
    }

    fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
        let rgb = img.to_rgb8();
        let mut out = Vec::new();
        let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
        enc.encode_image(&rgb).ok()?;
        Some(out)
    }

    pub fn compress_image_bytes_to_budget(
        raw_bytes: &[u8],
        max_bytes: usize,
    ) -> Option<(Vec<u8>, &'static str)> {
        if raw_bytes.is_empty() || max_bytes == 0 {
            return None;
        }
        let img = image::load_from_memory(raw_bytes).ok()?;
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let longest = w.max(h);
        for dim in [0u32, 1920, 1440, 1280] {
            if dim > 0 && longest <= dim {
                continue;
            }
            let scaled = scale_image(&img, dim);
            if let Some(bytes) = encode_png(&scaled) {
                if bytes.len() <= max_bytes {
                    return Some((bytes, ".png"));
                }
            }
        }
        for q in [85u8, 72, 55] {
            for dim in [0u32, 1920, 1440, 1280, 1024, 768, 512] {
                if (dim == 0 && longest > 1920) || (dim > 0 && longest <= dim) {
                    continue;
                }
                let scaled = scale_image(&img, dim);
                if let Some(bytes) = encode_jpeg(&scaled, q) {
                    if bytes.len() <= max_bytes {
                        return Some((bytes, ".jpg"));
                    }
                }
            }
        }
        for dim in [1024u32, 768, 512, 384] {
            if longest > dim {
                let scaled = scale_image(&img, dim);
                if let Some(bytes) = encode_png(&scaled) {
                    if bytes.len() <= max_bytes {
                        return Some((bytes, ".png"));
                    }
                }
            }
        }
        None
    }

    pub fn make_thumbnail_data_url(raw_bytes: &[u8], max_dim: u32) -> Option<String> {
        if raw_bytes.is_empty() {
            return None;
        }
        let img = image::load_from_memory(raw_bytes).ok()?;
        let thumb = if max_dim > 0 {
            img.thumbnail(max_dim, max_dim)
        } else {
            img
        };
        let png = encode_png(&thumb)?;
        Some(format!("data:image/png;base64,{}", B64.encode(&png)))
    }

    pub fn dib_bytes_to_png(_dib_bytes: &[u8]) -> Result<Vec<u8>, String> {
        Err("Windows DIB 形式はこの OS では未対応です".into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn pure_rust_thumbnail_and_compression_work() {
            let minimal_png = [
                0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I',
                b'H', b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
                0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, b'I', b'D',
                b'A', b'T', 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D,
                0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xAE, 0x42,
                0x60, 0x82,
            ];
            let url = make_thumbnail_data_url(&minimal_png, 64).unwrap();
            assert!(url.starts_with("data:image/png;base64,"));
            let (out, ext) = compress_image_bytes_to_budget(&minimal_png, 4096).unwrap();
            assert!(!out.is_empty());
            assert_eq!(ext, ".png");
        }
    }
}

#[cfg(not(windows))]
pub use platform::*;

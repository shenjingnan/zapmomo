/// 麦克风音频采集与重采样。
///
/// 用 cpal 采集麦克风（系统音频线程），把各采样格式转成 mono f32 后经
/// `mpsc` 发送；消费者在调用线程内用 sherpa-onnx 的 `LinearResampler` 把设备
/// 采样率（macOS 常见 44.1k/48k）重采样到模型要求的 16k。
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Sample, SampleFormat, SizedSample, StreamConfig};
use sherpa_onnx::LinearResampler;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// 麦克风句柄：持有 `cpal::Stream`（保持回调存活），并提供阻塞收块。
pub struct MicHandle {
    _stream: cpal::Stream,
    rx: mpsc::Receiver<Vec<f32>>,
    device_sample_rate: u32,
}

impl MicHandle {
    /// 设备实际采样率（macOS 常见 48000 / 44100）。
    pub fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }

    /// 阻塞接收一个原始 mono f32 块（设备采样率）。
    /// 返回 `None` 表示采集结束/出错。
    pub fn recv_chunk(&mut self) -> Option<Vec<f32>> {
        self.rx.recv().ok()
    }

    /// 带超时接收。`Err(Timeout)` = 超时（无新数据），`Err(Disconnected)` = 通道关闭。
    pub fn recv_chunk_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Vec<f32>, std::sync::mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

/// 启动麦克风采集。
///
/// `device_name` 为 `None` 时用系统默认输入设备；否则按名字「包含」匹配。
pub fn start_capture(device_name: Option<&str>) -> Result<MicHandle, String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => host
            .input_devices()
            .map_err(|e| format!("无法枚举输入设备: {e}"))?
            .find(|d| {
                d.description()
                    .map(|desc| desc.name().contains(name))
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("未找到输入设备: {name}（可用 kws list-devices 查看）"))?,
        None => host
            .default_input_device()
            .ok_or_else(mic_permission_hint)?,
    };

    let supported = device
        .default_input_config()
        .map_err(|e| format!("无法获取默认输入配置: {e}"))?;
    let config = supported.config();
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate;
    let (tx, rx) = mpsc::channel::<Vec<f32>>();

    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, tx, channels)?,
        SampleFormat::I16 => build_stream::<i16>(&device, &config, tx, channels)?,
        SampleFormat::U16 => build_stream::<u16>(&device, &config, tx, channels)?,
        other => return Err(format!("不支持的采样格式: {other:?}")),
    };
    stream.play().map_err(|e| format!("无法启动采集流: {e}"))?;

    Ok(MicHandle {
        _stream: stream,
        rx,
        device_sample_rate: sample_rate,
    })
}

/// 列出可用输入设备名（用于 `--device` 提示）。
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().map(|desc| desc.name().to_string()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 按采样格式分发，构建输入流。回调只做「解交织 + 取均值转 mono f32 + 发送」，
/// 全部使用 `std` 类型（回调运行在系统音频线程，必须能 `Send`）。
fn build_stream<T>(
    device: &Device,
    config: &StreamConfig,
    tx: mpsc::Sender<Vec<f32>>,
    channels: usize,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Sample<Float = f32>,
{
    let err_fn = |e: cpal::Error| eprintln!("[audio] 采集错误: {e}");
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                let mono = to_mono_f32(data, channels);
                // 消费者退出后忽略发送错误
                let _ = tx.send(mono);
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("无法建立输入流: {e}"))
}

/// 把交错声道数据转成 mono f32（取各声道均值）。
/// `Sample::to_float_sample()` 已把 i16/u16 归一化到 [-1, 1]。
pub(crate) fn to_mono_f32<T: Sample<Float = f32>>(interleaved: &[T], channels: usize) -> Vec<f32> {
    interleaved
        .chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|s| s.to_float_sample()).sum();
            sum / channels as f32
        })
        .collect()
}

/// 包装 sherpa-onnx 线性重采样器（消费者侧使用）。
pub struct Resampler {
    inner: LinearResampler,
}

impl Resampler {
    /// 从 `input_rate` 重采样到 `output_rate`。
    pub fn new(input_rate: i32, output_rate: i32) -> Result<Self, String> {
        let inner = LinearResampler::create(input_rate, output_rate)
            .ok_or_else(|| format!("无法创建重采样器: {input_rate} -> {output_rate}"))?;
        Ok(Self { inner })
    }

    /// 处理一帧音频。`flush=true` 处理最后一块以清空内部缓冲。
    pub fn process(&mut self, input: &[f32], flush: bool) -> Vec<f32> {
        self.inner.resample(input, flush)
    }
}

/// 录制 N 秒麦克风并保存为 16k mono PCM wav。
///
/// 返回 wav 路径（位于 `~/.zapmomo/tts/`）。采集复用 `start_capture`，
/// 在调用线程按截止时间循环收块，重采样到 16k 后写入。
pub fn record_voice(seconds: u32, device: Option<&str>) -> Result<PathBuf, String> {
    let out_dir = crate::config::settings::get_tts_output_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建录音目录失败: {e}"))?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out_path = out_dir.join(format!("rec-{millis}.wav"));

    let mut mic = start_capture(device)?;
    let src_rate = mic.device_sample_rate();
    let mut resampler = Resampler::new(src_rate as i32, 16_000)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds as u64);
    let mut samples: Vec<i16> = Vec::new();

    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        match mic.recv_chunk_timeout(std::time::Duration::from_millis(50)) {
            Ok(chunk) => {
                let out = resampler.process(&chunk, false);
                samples.extend(out.iter().map(|&s| f32_to_i16(s)));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("麦克风采集已断开".to_string());
            }
        }
    }
    // 冲刷重采样器尾部缓冲
    let tail = resampler.process(&[], true);
    samples.extend(tail.iter().map(|&s| f32_to_i16(s)));

    if samples.is_empty() {
        return Err("未采集到有效音频".to_string());
    }
    write_wav(&out_path, 16_000, &samples)?;
    Ok(out_path)
}

/// f32 归一化样本转 i16 PCM（裁剪到 [-1, 1]）。
fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// 写 16-bit PCM mono wav（最小 RIFF/WAVE 头，无第三方依赖）。
fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> Result<(), String> {
    use std::io::Write;
    let data_len = samples.len() as u32 * 2;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).map_err(|e| format!("创建录音文件失败: {e}"))?;
    f.write_all(&buf)
        .map_err(|e| format!("写入录音失败: {e}"))?;
    Ok(())
}

/// 把 f32 归一化样本写成 16-bit PCM mono wav（用于语速重采样后的合成输出）。
pub fn write_wav_f32(path: &Path, sample_rate: u32, samples: &[f32]) -> Result<(), String> {
    let pcm: Vec<i16> = samples.iter().map(|&s| f32_to_i16(s)).collect();
    write_wav(path, sample_rate, &pcm)
}

/// 读取 wav 文件为（mono f32 样本, 采样率）。
///
/// 文件不存在 / 非 RIFF / 编码不支持一律返回 `None`（上游 sherpa `Wave::read`
/// 不区分失败原因）；采样率不做归一，由调用方按需重采样。
pub fn read_wav_samples(path: &Path) -> Option<(Vec<f32>, i32)> {
    let wave = sherpa_onnx::Wave::read(&path.to_string_lossy())?;
    Some((wave.samples().to_vec(), wave.sample_rate()))
}

fn mic_permission_hint() -> String {
    "未找到默认麦克风。\n  macOS 请在「系统设置 → 隐私与安全性 → 麦克风」中授权当前终端 App，然后重试。\n  可用 `kws list-devices` 查看设备，`kws run --device <名称>` 指定设备。"
        .to_string()
}

/// 请求 macOS 麦克风授权（弹出系统授权窗），返回是否已授权。
///
/// macOS 上未授权时 CoreAudio 会隐藏输入设备，导致 [`list_input_devices`] /
/// [`start_capture`] 枚举为空；必须显式调用 AVCaptureDevice 的
/// `requestAccessForMediaType:` 弹窗授权后设备才可见。授权绑定当前可执行文件的
/// 签名（CDHash），调试模式下每次重新编译后授权会失效，需再次请求。
///
/// 返回：`Ok(true)` 已授权；`Ok(false)` 用户拒绝或系统限制；`Err` 请求失败。
/// 非 macOS 平台无需显式授权，恒返回 `Ok(true)`。
#[cfg(target_os = "macos")]
pub fn request_mic_permission() -> Result<bool, String> {
    use block2::RcBlock;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use std::sync::mpsc;
    use std::time::Duration;

    autoreleasepool(|_| {
        let media_type = unsafe { AVMediaTypeAudio }
            .ok_or_else(|| "AVFoundation 未加载音频媒体类型常量".to_string())?;
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
        match status {
            AVAuthorizationStatus::Authorized => Ok(true),
            AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => Ok(false),
            AVAuthorizationStatus::NotDetermined => {
                let (tx, rx) = mpsc::channel::<bool>();
                let block = RcBlock::new(move |granted: Bool| {
                    let _ = tx.send(granted.as_bool());
                });
                unsafe {
                    AVCaptureDevice::requestAccessForMediaType_completionHandler(
                        media_type, &block,
                    );
                }
                match rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(granted) => Ok(granted),
                    Err(_) => Err("等待麦克风授权结果超时".to_string()),
                }
            }
            other => Err(format!("未知麦克风权限状态: {other:?}")),
        }
    })
}

/// 非 macOS 平台无系统级麦克风授权机制，视为已授权。
#[cfg(not(target_os = "macos"))]
pub fn request_mic_permission() -> Result<bool, String> {
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_mono_f32_mono() {
        let data = [1.0f32, -1.0, 0.5];
        let mono = to_mono_f32(&data, 1);
        assert_eq!(mono, vec![1.0, -1.0, 0.5]);
    }

    #[test]
    fn test_to_mono_f32_stereo_average() {
        let data = [0.2f32, 0.4, 0.6, 1.0];
        let mono = to_mono_f32(&data, 2);
        assert!((mono[0] - 0.3).abs() < 1e-6);
        assert!((mono[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_to_mono_f32_i16_normalized() {
        let data = [i16::MAX, 0, i16::MIN];
        let mono = to_mono_f32(&data, 1);
        assert!((mono[0] - 1.0).abs() < 1e-4);
        assert_eq!(mono[1], 0.0);
        assert!((mono[2] - (-1.0)).abs() < 1e-4);
    }

    #[test]
    fn test_to_mono_f32_u16_normalized() {
        // 0x8000 = 32768 为静音中点，应映射为 0.0
        let data = [u16::MAX, 0x8000, 0];
        let mono = to_mono_f32(&data, 1);
        assert!((mono[0] - 1.0).abs() < 1e-4);
        assert_eq!(mono[1], 0.0);
        assert!((mono[2] - (-1.0)).abs() < 1e-4);
    }

    #[test]
    fn test_resampler_identity_same_rate() {
        let mut rs = Resampler::new(16000, 16000).unwrap();
        let input: Vec<f32> = vec![0.1; 3200];
        let out = rs.process(&input, true);
        // 同采样率线性重采样应接近原长度
        assert!(
            (out.len() as i64 - 3200).abs() <= 64,
            "identity resample len={}",
            out.len()
        );
    }

    #[test]
    fn test_resampler_downsample_48k_to_16k() {
        let mut rs = Resampler::new(48000, 16000).unwrap();
        assert_eq!(rs.inner.input_sample_rate(), 48000);
        let input: Vec<f32> = vec![0.0; 48000]; // 1 秒
        let out = rs.process(&input, true);
        // 48k -> 16k，1 秒应约 16000 个样本
        assert!(
            (out.len() as i64 - 16000).abs() <= 64,
            "downsample len={}",
            out.len()
        );
    }

    #[test]
    fn test_write_wav_header_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.wav");
        let samples = [0i16, i16::MAX, i16::MIN, 1234];
        write_wav(&path, 16000, &samples).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[24..28], &16000u32.to_le_bytes());
        assert_eq!(&bytes[34..36], &16u16.to_le_bytes()); // 16-bit
        // data chunk 长度 = 样本数 * 2
        assert_eq!(&bytes[40..44], &(samples.len() as u32 * 2).to_le_bytes());
        // 样本按 little-endian 写入
        assert_eq!(&bytes[44..46], &samples[0].to_le_bytes());
        assert_eq!(&bytes[46..48], &samples[1].to_le_bytes());
    }

    #[test]
    fn test_f32_to_i16_clamps() {
        // 对称映射：f32 [-1,1] × 32767 → i16（-1.0 映射到 -32767，非 i16::MIN）
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX);
        assert_eq!(f32_to_i16(2.0), i16::MAX); // 超界裁剪
        assert_eq!(f32_to_i16(-2.0), -i16::MAX);
        assert_eq!(f32_to_i16(0.0), 0);
    }
}

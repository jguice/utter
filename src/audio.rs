//! Cross-platform audio capture via cpal.
//!
//! macOS and Linux share the same cpal-based capture flow. The only
//! platform-specific detail is that macOS needs `spawn_blocking` around the
//! blocking `stop_audio` call, while Linux can call it directly.

use anyhow::{anyhow, Context, Result};

const TARGET_SAMPLE_RATE: u32 = 16_000;

pub struct AudioCapture {
    shared: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    stop_tx: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<Result<u32>>, // returns source sample rate
    channels: u16,
}

fn build_input_stream(
    target: &std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    channels_out: &mut u16,
    rate_out: &mut u32,
) -> Result<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))?;
    let supported = device
        .default_input_config()
        .context("query default input config")?;
    let sample_format = supported.sample_format();
    let channels = supported.channels();
    let rate = supported.sample_rate().0;
    *channels_out = channels;
    *rate_out = rate;
    let config: cpal::StreamConfig = supported.clone().into();

    log::info!(
        "cpal input: device={:?} rate={} channels={} format={:?}",
        device.name().ok(),
        rate,
        channels,
        sample_format
    );

    let buf = target.clone();
    let err_fn = |e| log::warn!("cpal stream error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend_from_slice(data);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend(data.iter().map(|&s| s as f32 / 32768.0));
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                if let Ok(mut v) = buf.lock() {
                    v.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                }
            },
            err_fn,
            None,
        ),
        other => return Err(anyhow!("unsupported cpal sample format: {other:?}")),
    }
    .context("cpal build_input_stream")?;
    Ok(stream)
}

pub async fn start_audio() -> Result<AudioCapture> {
    let shared: std::sync::Arc<std::sync::Mutex<Vec<f32>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(16_000 * 10)));
    let shared_for_thread = shared.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(u16, u32)>>();

    let thread = std::thread::spawn(move || -> Result<u32> {
        let mut channels = 0u16;
        let mut rate = 0u32;
        let stream = match build_input_stream(&shared_for_thread, &mut channels, &mut rate) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("{e:#}")));
                return Err(e);
            }
        };
        use cpal::traits::StreamTrait;
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(anyhow!("stream.play: {e}")));
            return Err(anyhow!("stream.play: {e}"));
        }
        let _ = ready_tx.send(Ok((channels, rate)));

        // Blocking recv from the std mpsc stop channel. cpal callbacks run
        // on its own thread; this thread just parks until told to stop,
        // then drops the stream which triggers teardown.
        let _ = stop_rx.recv();
        drop(stream);
        Ok(rate)
    });

    let (channels, _rate) = ready_rx
        .recv()
        .map_err(|_| anyhow!("cpal thread died before sending ready"))??;

    Ok(AudioCapture {
        shared,
        stop_tx,
        thread,
        channels,
    })
}

pub async fn stop_audio(capture: AudioCapture) -> Result<Vec<f32>> {
    let AudioCapture { shared, stop_tx, thread, channels } = capture;
    let _ = stop_tx.send(());
    let rate = tokio::task::spawn_blocking(move || thread.join())
        .await
        .map_err(|e| anyhow!("join cpal thread: {e:#}"))?
        .map_err(|_| anyhow!("cpal thread panicked"))??;
    let mut samples = {
        let locked = shared.lock().map_err(|e| anyhow!("lock samples: {e}"))?;
        locked.clone()
    };
    // Downmix to mono if the device captured stereo+.
    if channels > 1 {
        let c = channels as usize;
        let mut mono = Vec::with_capacity(samples.len() / c);
        for chunk in samples.chunks_exact(c) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / c as f32);
        }
        samples = mono;
    }
    // Naive downsample to 16 kHz — pick every Nth sample. Sufficient for a
    // short-utterance PoC; upgrade to rubato or similar if quality matters.
    if rate != TARGET_SAMPLE_RATE && rate > 0 {
        let ratio = rate as f64 / TARGET_SAMPLE_RATE as f64;
        let out_len = (samples.len() as f64 / ratio) as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src = (i as f64 * ratio) as usize;
            if src < samples.len() {
                out.push(samples[src]);
            }
        }
        samples = out;
    }
    Ok(samples)
}

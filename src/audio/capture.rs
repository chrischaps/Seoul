use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use tracing::{info, warn};

pub const RING_CAPACITY: usize = 16_384;

type Consumer = ringbuf::HeapCons<f32>;
type ProdHeap = ringbuf::HeapProd<f32>;

pub struct CaptureHandle {
    pub stream: Stream,
    pub consumer: Consumer,
    pub sample_rate: u32,
}

pub fn start_capture() -> Result<CaptureHandle> {
    let host = cpal::host_from_id(cpal::HostId::Wasapi).context("WASAPI host not available")?;
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device for loopback"))?;
    // For WASAPI loopback, use the device's output config and pass it to
    // build_input_stream. cpal's WASAPI backend detects the "input stream on
    // output device" pattern and enables AUDCLNT_STREAMFLAGS_LOOPBACK.
    let config = device
        .default_output_config()
        .context("default_output_config failed on default output device")?;

    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let sample_format = config.sample_format();

    info!(
        device = %device.name().unwrap_or_else(|_| "?".into()),
        sample_rate,
        channels,
        ?sample_format,
        "WASAPI loopback capture starting"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (mut producer, consumer) = rb.split();

    let stream_config: cpal::StreamConfig = config.clone().into();
    let err_fn = |err| warn!(?err, "audio stream error");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| push_mono_f32(&mut producer, data, channels),
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| push_mono_i16(&mut producer, data, channels),
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| push_mono_u16(&mut producer, data, channels),
            err_fn,
            None,
        )?,
        other => return Err(anyhow!("unsupported sample format: {other:?}")),
    };

    stream.play().context("stream.play() failed")?;

    Ok(CaptureHandle {
        stream,
        consumer,
        sample_rate,
    })
}

#[inline]
fn push_mono_f32(producer: &mut ProdHeap, data: &[f32], channels: u16) {
    let ch = channels as usize;
    if ch <= 1 {
        producer.push_slice(data);
        return;
    }
    for frame in data.chunks_exact(ch) {
        let mut sum = 0.0f32;
        for s in frame {
            sum += *s;
        }
        if producer.try_push(sum / ch as f32).is_err() {
            break;
        }
    }
}

#[inline]
fn push_mono_i16(producer: &mut ProdHeap, data: &[i16], channels: u16) {
    let ch = channels as usize;
    let scale = 1.0 / i16::MAX as f32;
    for frame in data.chunks_exact(ch) {
        let mut sum = 0.0f32;
        for s in frame {
            sum += *s as f32 * scale;
        }
        if producer.try_push(sum / ch as f32).is_err() {
            break;
        }
    }
}

#[inline]
fn push_mono_u16(producer: &mut ProdHeap, data: &[u16], channels: u16) {
    let ch = channels as usize;
    for frame in data.chunks_exact(ch) {
        let mut sum = 0.0f32;
        for s in frame {
            sum += (*s as f32 - 32768.0) / 32768.0;
        }
        if producer.try_push(sum / ch as f32).is_err() {
            break;
        }
    }
}

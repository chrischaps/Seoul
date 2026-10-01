use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, I24, Sample, SampleFormat, SizedSample, Stream};
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

    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let sample_format = config.sample_format();

    info!(
        device = %device,
        sample_rate,
        channels,
        ?sample_format,
        "WASAPI loopback capture starting"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, consumer) = rb.split();

    let stream_config: cpal::StreamConfig = config.into();

    // cpal 0.18 can report I32/F64/I24 defaults on high-precision hardware,
    // so dispatch every format WASAPI may hand us to one generic downmixer.
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, stream_config, producer, channels)?,
        SampleFormat::F64 => build::<f64>(&device, stream_config, producer, channels)?,
        SampleFormat::I16 => build::<i16>(&device, stream_config, producer, channels)?,
        SampleFormat::I24 => build::<I24>(&device, stream_config, producer, channels)?,
        SampleFormat::I32 => build::<i32>(&device, stream_config, producer, channels)?,
        SampleFormat::U16 => build::<u16>(&device, stream_config, producer, channels)?,
        other => return Err(anyhow!("unsupported sample format: {other:?}")),
    };

    stream.play().context("stream.play() failed")?;

    Ok(CaptureHandle {
        stream,
        consumer,
        sample_rate,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: ProdHeap,
    channels: u16,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let err_fn = |err| warn!(?err, "audio stream error");
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| push_mono(&mut producer, data, channels),
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Downmix interleaved frames to mono f32 and push into the ring. Drops the
/// remainder of the callback if the analysis thread has fallen behind.
#[inline]
fn push_mono<T>(producer: &mut ProdHeap, data: &[T], channels: u16)
where
    T: Sample,
    f32: FromSample<T>,
{
    let ch = channels.max(1) as usize;
    let inv = 1.0 / ch as f32;
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
        if producer.try_push(sum * inv).is_err() {
            break;
        }
    }
}

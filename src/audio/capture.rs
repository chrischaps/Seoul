//! Live audio capture with automatic reconnection.
//!
//! [`Capture`] lives on the main thread (cpal streams are `!Send`) and is
//! polled once per frame. It listens to a [`Source`]: loopback on the
//! default output device (what the computer is playing), or an input device
//! (a microphone or line-in). It (re)opens the stream whenever there is
//! none, the stream reports an error, or the default device changes — e.g.
//! plugging in headphones — and hands each new sample ring to the analysis
//! thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, ErrorKind, FromSample, I24, Sample, SampleFormat, SizedSample, Stream};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use tracing::{debug, info, warn};

use crate::audio::analysis::AudioSource;

pub const RING_CAPACITY: usize = 16_384;

const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Give the OS a moment to settle after a device change before reopening.
const RECONNECT_DELAY: Duration = Duration::from_millis(250);

type ProdHeap = ringbuf::HeapProd<f32>;

struct Active {
    _stream: Stream,
    device_id: Option<DeviceId>,
    device_name: String,
    failed: Arc<AtomicBool>,
}

/// Which device a [`Capture`] listens to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// What the computer is playing: loopback on the default output.
    Loopback,
    /// A microphone or line input: the default one, or the first whose name
    /// contains this text (case-insensitive).
    Input(Option<String>),
}

pub struct Capture {
    host: Option<cpal::Host>,
    active: Option<Active>,
    source: Source,
    /// Ask the analyzer for room tuning (`Analyzer::set_room`).
    room: bool,
    sink: Sender<AudioSource>,
    next_attempt: Instant,
    next_default_check: Instant,
    warned_unavailable: bool,
}

impl Capture {
    /// Create the manager and try to open capture immediately. Never fails:
    /// with no usable device the visualizer simply runs on silence and keeps
    /// retrying in the background.
    pub fn new(sink: Sender<AudioSource>, source: Source, room: bool) -> Self {
        // WASAPI on Windows, CoreAudio on macOS.
        let host = Some(cpal::default_host());
        let now = Instant::now();
        let mut capture = Self {
            host,
            active: None,
            source,
            room,
            sink,
            next_attempt: now,
            next_default_check: now + DEFAULT_CHECK_INTERVAL,
            warned_unavailable: false,
        };
        capture.poll();
        capture
    }

    /// Name of the device currently being captured, if any.
    pub fn device_name(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.device_name.as_str())
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Switch devices (e.g. after a config reload). Reopens on the next poll
    /// only if something changed.
    pub fn set_source(&mut self, source: Source, room: bool) {
        if source != self.source || room != self.room {
            info!(?source, room, "audio source changed");
            self.source = source;
            self.room = room;
            self.active = None;
            self.warned_unavailable = false;
            self.next_attempt = Instant::now();
        }
    }

    pub fn poll(&mut self) {
        let Some(host) = self.host.as_ref() else {
            return;
        };
        let now = Instant::now();

        if let Some(active) = &self.active {
            let mut reconnect = false;
            if active.failed.load(Ordering::Relaxed) {
                info!(device = active.device_name, "audio stream lost, reconnecting");
                reconnect = true;
            } else if now >= self.next_default_check {
                self.next_default_check = now + DEFAULT_CHECK_INTERVAL;
                // A device picked by name stays put; only "the default" moves.
                let current = match &self.source {
                    Source::Loopback => Some(host.default_output_device()),
                    Source::Input(None) => Some(host.default_input_device()),
                    Source::Input(Some(_)) => None,
                };
                if let Some(current) = current
                    && current.and_then(|d| d.id().ok()) != active.device_id
                {
                    info!(from = active.device_name, "default audio device changed, reconnecting");
                    reconnect = true;
                }
            }
            if reconnect {
                self.active = None;
                self.next_attempt = now + RECONNECT_DELAY;
            }
        }

        if self.active.is_none() && now >= self.next_attempt {
            match open(host, &self.source, self.room) {
                Ok((active, source)) => {
                    // If the analysis thread is gone we're shutting down anyway.
                    let _ = self.sink.send(source);
                    self.active = Some(active);
                    self.warned_unavailable = false;
                    self.next_default_check = now + DEFAULT_CHECK_INTERVAL;
                }
                Err(e) => {
                    if !self.warned_unavailable {
                        warn!("audio capture unavailable, retrying every {RETRY_INTERVAL:?}: {e:#}");
                        self.warned_unavailable = true;
                    } else {
                        debug!("audio capture retry failed: {e:#}");
                    }
                    self.next_attempt = now + RETRY_INTERVAL;
                }
            }
        }
    }
}

/// Names of the input devices the default host offers, each flagged true
/// if it is the system default (listed first).
pub fn input_device_names() -> Result<Vec<(String, bool)>> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|d| d.to_string());
    let mut names: Vec<(String, bool)> = host
        .input_devices()?
        .map(|d| d.to_string())
        .map(|n| (n.clone(), default.as_deref() == Some(n.as_str())))
        .collect();
    names.sort_by_key(|(_, is_default)| !is_default);
    Ok(names)
}

fn find_input(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    let want = name.to_lowercase();
    host.input_devices()?
        .find(|d| d.to_string().to_lowercase().contains(&want))
        .ok_or_else(|| anyhow!("no input device matching '{name}'"))
}

fn open(host: &cpal::Host, source: &Source, room: bool) -> Result<(Active, AudioSource)> {
    let (device, config) = match source {
        Source::Loopback => {
            let device = host
                .default_output_device()
                .ok_or_else(|| anyhow!("no default output device for loopback"))?;
            // Loopback = the device's *output* config passed to
            // build_input_stream. cpal's WASAPI backend sees an input stream
            // on a render device and sets AUDCLNT_STREAMFLAGS_LOOPBACK; its
            // CoreAudio backend (macOS 14.6+) sees a device with no inputs
            // and records it through a process tap.
            let config = device
                .default_output_config()
                .context("default_output_config failed on default output device")?;
            (device, config)
        }
        Source::Input(name) => {
            let device = match name {
                Some(name) => find_input(host, name)?,
                None => host.default_input_device().ok_or_else(|| anyhow!("no default input device"))?,
            };
            let config = device
                .default_input_config()
                .context("default_input_config failed on input device")?;
            (device, config)
        }
    };

    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let sample_format = config.sample_format();
    let device_name = device.to_string();

    info!(
        device = %device_name,
        sample_rate,
        channels,
        ?sample_format,
        ?source,
        "audio capture starting"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, consumer) = rb.split();
    let failed = Arc::new(AtomicBool::new(false));

    let stream_config: cpal::StreamConfig = config.into();
    let f = failed.clone();
    // cpal 0.18 can report I32/F64/I24 defaults on high-precision hardware,
    // so dispatch every format WASAPI may hand us to one generic downmixer.
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, stream_config, producer, channels, f)?,
        SampleFormat::F64 => build::<f64>(&device, stream_config, producer, channels, f)?,
        SampleFormat::I16 => build::<i16>(&device, stream_config, producer, channels, f)?,
        SampleFormat::I24 => build::<I24>(&device, stream_config, producer, channels, f)?,
        SampleFormat::I32 => build::<i32>(&device, stream_config, producer, channels, f)?,
        SampleFormat::U16 => build::<u16>(&device, stream_config, producer, channels, f)?,
        other => return Err(anyhow!("unsupported sample format: {other:?}")),
    };

    stream.play().context("stream.play() failed")?;

    Ok((
        Active {
            _stream: stream,
            device_id: device.id().ok(),
            device_name,
            failed,
        },
        AudioSource {
            consumer,
            sample_rate,
            room,
        },
    ))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: ProdHeap,
    channels: u16,
    failed: Arc<AtomicBool>,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let err_fn = move |err: cpal::Error| match err.kind() {
        // Overruns are recoverable glitches, not a dead stream.
        ErrorKind::Xrun => debug!(?err, "audio capture xrun"),
        _ => {
            warn!(?err, "audio stream error");
            failed.store(true, Ordering::Relaxed);
        }
    };
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

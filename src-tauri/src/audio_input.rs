//! CPAL input capture hub — rings, meters, monitoring mix (ADR-027).
//!
//! The input callback only writes peaks and pushes stereo floats into two SPSC
//! rings. A non-RT drain thread pulls the record ring; the output callback pulls
//! the monitor ring and mixes after the graph (like the metronome).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use crate::audio_engine::engine::{ChannelLevels, SendStream, MAX_BLOCK_FRAMES};

/// Stereo interleaved frames kept for ~4 s at 48 kHz (record path).
const RECORD_RING_SAMPLES: usize = 48_000 * 2 * 4;
/// A few blocks of stereo for software monitoring.
const MONITOR_RING_SAMPLES: usize = MAX_BLOCK_FRAMES * 2 * 8;

/// Lock-free SPSC ring of `f32` samples. Capacity is rounded up to a power of two.
pub struct SampleRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Single producer + single consumer — same contract as ParamQueue (ADR-007).
unsafe impl Sync for SampleRing {}

impl SampleRing {
    pub fn with_capacity(min_samples: usize) -> Self {
        let capacity = min_samples.next_power_of_two().max(2);
        Self {
            buf: (0..capacity)
                .map(|_| UnsafeCell::new(0.0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer. Returns how many samples were written (drops the rest on full).
    pub fn push_slice(&self, samples: &[f32]) -> usize {
        let mut written = 0;
        for &sample in samples {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) > self.mask {
                break;
            }
            unsafe {
                *self.buf[tail & self.mask].get() = sample;
            }
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
            written += 1;
        }
        written
    }

    /// Consumer. Returns how many samples were read.
    pub fn pop_slice(&self, out: &mut [f32]) -> usize {
        let mut read = 0;
        for slot in out.iter_mut() {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            unsafe {
                *slot = *self.buf[head & self.mask].get();
            }
            self.head.store(head.wrapping_add(1), Ordering::Release);
            read += 1;
        }
        read
    }

    pub fn clear(&self) {
        let tail = self.tail.load(Ordering::Acquire);
        self.head.store(tail, Ordering::Release);
    }
}

/// Shared input capture state (command thread + both CPAL callbacks + drain).
pub struct AudioInputHub {
    pub record_ring: SampleRing,
    pub monitor_ring: SampleRing,
    pub monitor_enabled: AtomicBool,
    /// Non-zero when an input stream is live (sample rate of the open input).
    pub sample_rate_bits: AtomicU32,
    pub available: AtomicBool,
}

impl AudioInputHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            record_ring: SampleRing::with_capacity(RECORD_RING_SAMPLES),
            monitor_ring: SampleRing::with_capacity(MONITOR_RING_SAMPLES),
            monitor_enabled: AtomicBool::new(false),
            sample_rate_bits: AtomicU32::new(0),
            available: AtomicBool::new(false),
        })
    }

    pub fn set_monitor_enabled(&self, enabled: bool) {
        self.monitor_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn monitor_enabled(&self) -> bool {
        self.monitor_enabled.load(Ordering::Relaxed)
    }

    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate_bits.load(Ordering::Relaxed))
    }

    pub fn reset_rings(&self) {
        self.record_ring.clear();
        self.monitor_ring.clear();
    }
}

/// Open a CPAL input stream that feeds `hub` and `levels`. Returns `None` when
/// no device is available (output still runs).
pub fn open_input_stream(
    host: &cpal::Host,
    device_name: &str,
    requested_sr: u32,
    buffer_size: u32,
    hub: Arc<AudioInputHub>,
    levels: Arc<ChannelLevels>,
) -> Option<SendStream> {
    let device = find_input_device(host, device_name)?;
    let (base_cfg, fmt) = resolve_input_stream_config(&device, requested_sr).ok()?;
    let hw_channels = base_cfg.channels as usize;
    let actual_sr = base_cfg.sample_rate.0;

    eprintln!(
        "[audio] IN {} — {} Hz, {} ch, {:?}",
        device
            .name()
            .unwrap_or_else(|_| "Périphérique inconnu".to_string()),
        actual_sr,
        hw_channels,
        fmt,
    );

    hub.reset_rings();
    hub.sample_rate_bits
        .store((actual_sr as f32).to_bits(), Ordering::Relaxed);

    let fixed_cfg = cpal::StreamConfig {
        buffer_size: cpal::BufferSize::Fixed(buffer_size),
        ..base_cfg.clone()
    };

    let stream = build_typed_input_stream(&device, &fixed_cfg, fmt, hub.clone(), levels.clone())
        .or_else(|e| {
            eprintln!(
                "[audio] Buffer fixe input ({buffer_size}) non supporté ({e}), mode défaut"
            );
            let default_cfg = cpal::StreamConfig {
                buffer_size: cpal::BufferSize::Default,
                ..base_cfg
            };
            build_typed_input_stream(&device, &default_cfg, fmt, hub.clone(), levels)
        })
        .ok()?;

    if let Err(e) = stream.play() {
        eprintln!("[audio] Impossible de démarrer le flux d'entrée : {e}");
        hub.available.store(false, Ordering::Relaxed);
        hub.sample_rate_bits.store(0, Ordering::Relaxed);
        return None;
    }

    hub.available.store(true, Ordering::Relaxed);
    Some(SendStream(stream))
}

fn find_input_device(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    if name == "default" {
        return host.default_input_device();
    }
    if let Ok(mut devices) = host.input_devices() {
        if let Some(d) = devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
            return Some(d);
        }
    }
    eprintln!(
        "[audio] Entrée '{}' introuvable, périphérique d'entrée par défaut",
        name
    );
    host.default_input_device()
}

fn resolve_input_stream_config(
    device: &cpal::Device,
    requested_sr: u32,
) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let ranges: Vec<_> = device
        .supported_input_configs()
        .map(|it| it.collect::<Vec<_>>())
        .unwrap_or_default();

    let find = |sr: cpal::SampleRate, fmt: cpal::SampleFormat| {
        ranges
            .iter()
            .find(|r| {
                r.sample_format() == fmt && r.min_sample_rate() <= sr && r.max_sample_rate() >= sr
            })
            .map(|r| (r.with_sample_rate(sr).into(), fmt))
    };

    let req = cpal::SampleRate(requested_sr);
    for fmt in [cpal::SampleFormat::F32, cpal::SampleFormat::I16] {
        if let Some(found) = find(req, fmt) {
            return Ok(found);
        }
    }
    if let Some(range) = ranges
        .iter()
        .find(|r| r.min_sample_rate() <= req && r.max_sample_rate() >= req)
    {
        return Ok((range.with_sample_rate(req).into(), range.sample_format()));
    }

    let default_cfg = device
        .default_input_config()
        .map_err(|e| format!("Impossible d'obtenir la configuration d'entrée : {e}"))?;
    Ok((default_cfg.clone().into(), default_cfg.sample_format()))
}

fn build_typed_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: cpal::SampleFormat,
    hub: Arc<AudioInputHub>,
    levels: Arc<ChannelLevels>,
) -> Result<cpal::Stream, String> {
    match fmt {
        cpal::SampleFormat::F32 => build_input_stream::<f32>(device, config, hub, levels),
        cpal::SampleFormat::I16 => build_input_stream::<i16>(device, config, hub, levels),
        cpal::SampleFormat::U16 => build_input_stream::<u16>(device, config, hub, levels),
        other => {
            eprintln!("[audio] Format input {other:?} — tentative F32");
            build_input_stream::<f32>(device, config, hub, levels)
        }
    }
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    hub: Arc<AudioInputHub>,
    levels: Arc<ChannelLevels>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let hw_channels = config.channels as usize;
    // Scratch for stereo conversion — allocated once before the callback runs.
    let mut stereo = vec![0.0_f32; MAX_BLOCK_FRAMES * 2];

    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                let Some(frames) = data.len().checked_div(hw_channels) else {
                    return;
                };
                if frames == 0 || frames > MAX_BLOCK_FRAMES {
                    return;
                }

                let mut peak_l = 0.0_f32;
                let mut peak_r = 0.0_f32;
                for f in 0..frames {
                    let (l, r) = match hw_channels {
                        0 => (0.0, 0.0),
                        1 => {
                            let s = f32::from_sample_(data[f]);
                            (s, s)
                        }
                        _ => {
                            let base = f * hw_channels;
                            let l = f32::from_sample_(data[base]);
                            let r = f32::from_sample_(data[base + 1]);
                            (l, r)
                        }
                    };
                    stereo[f * 2] = l;
                    stereo[f * 2 + 1] = r;
                    peak_l = peak_l.max(l.abs());
                    peak_r = peak_r.max(r.abs());
                }

                levels.set_input(0, peak_l);
                levels.set_input(1, peak_r);

                let slice = &stereo[..frames * 2];
                let _ = hub.record_ring.push_slice(slice);
                if hub.monitor_enabled.load(Ordering::Relaxed) {
                    let _ = hub.monitor_ring.push_slice(slice);
                }
            },
            move |err| {
                eprintln!("[audio] Erreur de flux d'entrée : {err}");
            },
            None,
        )
        .map_err(|e| e.to_string())?;

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_round_trips_samples() {
        let ring = SampleRing::with_capacity(8);
        assert_eq!(ring.push_slice(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0; 4];
        assert_eq!(ring.pop_slice(&mut out), 3);
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn ring_drops_when_full() {
        let ring = SampleRing::with_capacity(4);
        // capacity 4 → mask 3 → usable slots = 4 (full when tail-head > mask)
        let written = ring.push_slice(&[1.0; 16]);
        assert!(written <= 4);
        assert!(written > 0);
    }
}

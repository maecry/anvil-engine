//! Benchmark recorder behind `ANVIL_BENCH=path.json` (ADR-0013 §1, `docs/benchmarks/`).
//!
//! An example creates a [`Bench`] with [`Bench::from_env`], calls [`Bench::record`] after each
//! rendered frame and exits once it returns `true`; the JSON is written then. Workloads must be
//! driven by the frame number so runs are comparable.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use rhi::{DeviceDiagnostics, DeviceInfo};

use crate::RendererStats;

/// Default warm-up frames (`ANVIL_BENCH_WARMUP`).
const WARMUP: u64 = 120;
/// Default measured frames (`ANVIL_BENCH_FRAMES`).
const FRAMES: u64 = 1000;

/// Collects per-frame samples and writes the result JSON.
pub struct Bench {
    path: String,
    workload: String,
    warmup: u64,
    frames: u64,
    gpu: DeviceInfo,
    last_tick: Option<Instant>,
    /// Device frame counter when measuring began; older GPU timings are warm-up.
    first_device_frame: u64,
    last_gpu_frame: u64,
    cpu: Vec<f64>,
    interval: Vec<f64>,
    gpu_total: Vec<f64>,
    passes: Vec<(String, Vec<f64>)>,
    stages: Vec<(&'static str, Vec<f64>)>,
    /// Instances re-uploaded per measured frame.
    uploaded: Vec<f64>,
    last: Option<(RendererStats, DeviceDiagnostics, (u32, u32))>,
}

/// `p` in 0..=1, nearest-rank on a sorted copy; 0 for no samples.
fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((p * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1]
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn stat_json(v: &[f64]) -> String {
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    format!(
        "{{\"median\": {:.4}, \"p99\": {:.4}, \"mean\": {mean:.4}, \"samples\": {}}}",
        percentile(v, 0.5),
        percentile(v, 0.99),
        v.len()
    )
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn first_line_of(path: &str, key: &str) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| {
            t.lines().find(|l| l.starts_with(key)).map(|l| {
                l.split_once(['=', ':'])
                    .map_or("", |x| x.1)
                    .trim()
                    .trim_matches('"')
                    .to_owned()
            })
        })
        .unwrap_or_default()
}

impl Bench {
    /// `Some` when `ANVIL_BENCH` is set. `ANVIL_BENCH_WARMUP` (default 120) and
    /// `ANVIL_BENCH_FRAMES` (default 1000) set the protocol.
    pub fn from_env(workload: &str, gpu: &DeviceInfo) -> Option<Self> {
        let path = std::env::var("ANVIL_BENCH")
            .ok()
            .filter(|p| !p.is_empty())?;
        let b = Self {
            path,
            workload: workload.to_owned(),
            warmup: env_u64("ANVIL_BENCH_WARMUP", WARMUP),
            frames: env_u64("ANVIL_BENCH_FRAMES", FRAMES).max(1),
            gpu: gpu.clone(),
            last_tick: None,
            first_device_frame: u64::MAX,
            last_gpu_frame: 0,
            cpu: Vec::new(),
            interval: Vec::new(),
            gpu_total: Vec::new(),
            passes: Vec::new(),
            stages: Vec::new(),
            uploaded: Vec::new(),
            last: None,
        };
        tracing::info!(path = %b.path, workload, warmup = b.warmup, frames = b.frames, "benchmark enabled");
        Some(b)
    }

    /// Frames to render in total (warm-up plus measured).
    pub fn total_frames(&self) -> u64 {
        self.warmup + self.frames
    }

    /// Records frame number `index` (counting rendered frames from 0). `cpu` is the app's time
    /// for the frame (begin to present); the frame interval is measured between calls. Returns
    /// `true` after the last measured frame, when the JSON has been written.
    pub fn record(
        &mut self,
        index: u64,
        cpu: Duration,
        stats: &RendererStats,
        diag: &DeviceDiagnostics,
        extent: (u32, u32),
    ) -> bool {
        let now = Instant::now();
        let tick = self.last_tick.replace(now);
        if index < self.warmup {
            return false;
        }
        if index == self.warmup {
            self.first_device_frame = diag.frame;
        }
        self.cpu.push(cpu.as_secs_f64() * 1e3);
        if let Some(t) = tick {
            self.interval.push((now - t).as_secs_f64() * 1e3);
        }
        let g = &stats.gpu;
        if !g.passes.is_empty()
            && g.frame != self.last_gpu_frame
            && g.frame >= self.first_device_frame
        {
            self.last_gpu_frame = g.frame;
            self.gpu_total.push(g.total_ms);
            for (name, ms) in &g.passes {
                match self.passes.iter_mut().find(|(n, _)| n == name.as_ref()) {
                    Some((_, v)) => v.push(*ms),
                    None => self.passes.push((name.to_string(), vec![*ms])),
                }
            }
        }
        let st = &stats.stages;
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let row = [
            ("pack", ms(st.pack)),
            ("cull", ms(st.cull)),
            ("sort", ms(st.sort)),
            ("shadow_lists", ms(st.shadow_lists)),
            ("clusters", ms(st.clusters)),
            ("upload", ms(st.upload)),
            ("graph_build", ms(st.graph_build)),
            ("compile", ms(st.compile)),
            ("execute", ms(st.execute)),
        ];
        if self.stages.is_empty() {
            self.stages = row.iter().map(|(n, _)| (*n, Vec::new())).collect();
        }
        for ((_, v), (_, x)) in self.stages.iter_mut().zip(row) {
            v.push(x);
        }
        self.uploaded.push(f64::from(stats.instances_uploaded));
        self.last = Some((stats.clone(), diag.clone(), extent));
        if index + 1 < self.total_frames() {
            return false;
        }
        match std::fs::write(&self.path, self.to_json()) {
            Ok(()) => tracing::info!(path = %self.path, "benchmark written"),
            Err(e) => tracing::error!(path = %self.path, error = %e, "benchmark not written"),
        }
        true
    }

    fn to_json(&self) -> String {
        let (stats, diag, extent) = self.last.clone().expect("at least one measured frame");
        let env: Vec<String> = {
            let mut v: Vec<_> = std::env::vars()
                .filter(|(k, _)| k.starts_with("ANVIL_"))
                .collect();
            v.sort();
            v.iter()
                .map(|(k, x)| format!("{}: {}", json_str(k), json_str(x)))
                .collect()
        };
        let rustc = std::process::Command::new("rustc")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        let cpu_model = first_line_of("/proc/cpuinfo", "model name");
        let os = first_line_of("/etc/os-release", "PRETTY_NAME");
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
        let (api, g) = (self.gpu.api_version, &self.gpu);
        let passes: Vec<String> = self
            .passes
            .iter()
            .map(|(n, v)| {
                format!(
                    "    {{\"name\": {}, \"median\": {:.4}, \"p99\": {:.4}}}",
                    json_str(n),
                    percentile(v, 0.5),
                    percentile(v, 0.99)
                )
            })
            .collect();
        let stages: Vec<String> = self
            .stages
            .iter()
            .map(|(n, v)| format!("    {}: {}", json_str(n), stat_json(v)))
            .collect();
        format!(
            "{{\n  \"schema\": 1,\n  \"workload\": {},\n  \"variant_env\": {{{}}},\n  \"machine\": {{\n    \"gpu\": {},\n    \"driver\": {},\n    \"vendor_id\": {},\n    \"vulkan_api\": \"{}.{}.{}\",\n    \"cpu\": {},\n    \"logical_cpus\": {},\n    \"worker_threads\": {},\n    \"os\": {},\n    \"kernel\": {},\n    \"rustc\": {},\n    \"build\": \"{}\"\n  }},\n  \"protocol\": {{\"warmup_frames\": {}, \"measured_frames\": {}, \"resolution\": [{}, {}]}},\n  \"cpu_frame_ms\": {},\n  \"frame_interval_ms\": {},\n  \"gpu_frame_ms\": {},\n  \"cpu_stages_ms\": {{\n{}\n  }},\n  \"gpu_passes_ms\": [\n{}\n  ],\n  \"draws\": {},\n  \"shadow_draws\": {},\n  \"triangles\": {},\n  \"lights\": {},\n  \"culled_instances\": {},\n  \"gpu_culling\": {},\n  \"instances\": {},\n  \"instances_uploaded\": {},\n  \"cull\": {{\"visible\": {}, \"frustum_culled\": {}, \"occlusion_culled\": {}, \"triangles\": {}, \"camera_draws\": {}, \"phase2_draws\": {}, \"shadow_draws\": {}}},\n  \"vram_bytes\": {{\"allocated\": {}, \"reserved\": {}}}\n}}\n",
            json_str(&self.workload),
            env.join(", "),
            json_str(&g.name),
            json_str(&g.driver),
            g.vendor_id,
            api.0,
            api.1,
            api.2,
            json_str(&cpu_model),
            std::thread::available_parallelism().map_or(0, |n| n.get()),
            foundation::init_thread_pool(),
            json_str(&os),
            json_str(kernel.trim()),
            json_str(&rustc),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            self.warmup,
            self.frames,
            extent.0,
            extent.1,
            stat_json(&self.cpu),
            stat_json(&self.interval),
            stat_json(&self.gpu_total),
            stages.join(",\n"),
            passes.join(",\n"),
            stats.draws,
            stats.shadow_draws,
            stats.triangles,
            stats.lights,
            stats.culled,
            stats.gpu_culling,
            stats.instances,
            stat_json(&self.uploaded),
            stats.cull.visible,
            stats.cull.frustum_culled,
            stats.cull.occlusion_culled,
            stats.cull.triangles,
            stats.cull.camera_draws,
            stats.cull.phase2_draws,
            stats.cull.shadow_draws,
            diag.memory.allocated_bytes,
            diag.memory.reserved_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_escaping() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), 50.0);
        assert_eq!(percentile(&v, 0.99), 99.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.99), 7.0);
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }
}

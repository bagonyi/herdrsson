//! Fork: the machine's CPU and memory use for the spaces header (`[ui.sidebar.spaces]
//! system_usage`). The server reads them itself every `interval_seconds`, on the tab bar status
//! timer: a few system calls, no process started (`crate::platform::system_usage_sample`). The
//! snapshot carries the latest figures to the clients, which draw them.

use std::time::{Duration, Instant};

use super::App;
use crate::platform::{MemoryPressure, SystemUsageSample};
use crate::protocol::{ClientShellMemoryPressure, ClientShellSystemUsage};

/// The first reading only starts the CPU count; the next, this soon after it, gives a figure.
const FIRST_CPU_FIGURE_AFTER: Duration = Duration::from_secs(1);

pub(super) struct SystemUsageRuntime {
    interval: Duration,
    next_at: Instant,
    previous: Option<SystemUsageSample>,
    current: Option<ClientShellSystemUsage>,
}

impl App {
    /// Reads the system usage every `config`'s interval from now on, or stops when there's no
    /// config or it has a problem. A reload keeps the figures shown and the CPU count going.
    pub(super) fn configure_system_usage(
        &mut self,
        config: Option<&crate::config::SystemUsageConfig>,
    ) {
        let Some(interval) = config
            .filter(|config| config.problems().is_empty())
            .map(|config| Duration::from_secs(config.interval_seconds))
        else {
            self.system_usage = None;
            return;
        };
        let now = Instant::now();
        match self.system_usage.as_mut() {
            Some(runtime) => {
                runtime.interval = interval;
                runtime.next_at = runtime.next_at.min(now + interval);
            }
            None => {
                self.system_usage = Some(SystemUsageRuntime {
                    interval,
                    next_at: now,
                    previous: None,
                    current: None,
                })
            }
        }
    }

    /// Reads the system usage when it's due; true when the figures changed.
    pub(super) fn refresh_system_usage(&mut self, now: Instant) -> bool {
        let Some(runtime) = self
            .system_usage
            .as_mut()
            .filter(|runtime| now >= runtime.next_at)
        else {
            return false;
        };
        let sample = crate::platform::system_usage_sample();
        let wait = if runtime.previous.is_none() && sample.is_some() {
            FIRST_CPU_FIGURE_AFTER.min(runtime.interval)
        } else {
            runtime.interval
        };
        runtime.next_at = now.checked_add(wait).unwrap_or(now);
        let next = sample.map(|sample| figures(runtime.previous.as_ref(), &sample));
        runtime.previous = sample;
        if next == runtime.current {
            return false;
        }
        runtime.current = next;
        true
    }

    pub(super) fn next_system_usage_deadline(&self) -> Option<Instant> {
        self.system_usage.as_ref().map(|runtime| runtime.next_at)
    }

    /// The latest figures, none while it's off or before the first reading.
    pub(crate) fn system_usage(&self) -> Option<ClientShellSystemUsage> {
        self.system_usage.as_ref()?.current
    }
}

/// `sample`'s figures, its CPU use since `previous` (none without one, or when no time passed).
fn figures(
    previous: Option<&SystemUsageSample>,
    sample: &SystemUsageSample,
) -> ClientShellSystemUsage {
    let cpu_percent = previous.and_then(|previous| {
        let busy = sample.cpu_ticks[0].wrapping_sub(previous.cpu_ticks[0]);
        let all = sample.cpu_ticks[1].wrapping_sub(previous.cpu_ticks[1]);
        let (busy, all) = (u64::from(busy.min(all)), u64::from(all));
        (all > 0).then(|| u8::try_from((busy * 100 + all / 2) / all).unwrap_or(100))
    });
    ClientShellSystemUsage {
        cpu_percent,
        memory_used_bytes: sample.memory_used_bytes,
        memory_total_bytes: sample.memory_total_bytes,
        memory_pressure: match sample.memory_pressure {
            Some(MemoryPressure::Normal) => ClientShellMemoryPressure::Normal,
            Some(MemoryPressure::Warning) => ClientShellMemoryPressure::Warning,
            Some(MemoryPressure::Critical) => ClientShellMemoryPressure::Critical,
            None => ClientShellMemoryPressure::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(busy: u32, all: u32) -> SystemUsageSample {
        SystemUsageSample {
            cpu_ticks: [busy, all],
            memory_used_bytes: 13 << 30,
            memory_total_bytes: 16 << 30,
            memory_pressure: Some(MemoryPressure::Warning),
        }
    }

    #[test]
    fn the_cpu_figure_is_the_busy_share_since_the_reading_before() {
        let first = figures(None, &sample(100, 1_000));
        assert_eq!(first.cpu_percent, None, "one reading has no figure yet");
        assert_eq!(first.memory_used_bytes, 13 << 30);
        assert_eq!(first.memory_pressure, ClientShellMemoryPressure::Warning);

        let busy = figures(Some(&sample(100, 1_000)), &sample(178, 1_100));
        assert_eq!(busy.cpu_percent, Some(78));
        let idle = figures(Some(&sample(100, 1_000)), &sample(100, 1_100));
        assert_eq!(idle.cpu_percent, Some(0));
        let same = figures(Some(&sample(100, 1_000)), &sample(100, 1_000));
        assert_eq!(same.cpu_percent, None, "no time passed");
    }

    #[test]
    fn the_cpu_figure_survives_the_kernel_counters_wrapping() {
        let before = sample(u32::MAX - 9, u32::MAX - 49);
        let after = sample(10, 50);
        assert_eq!(figures(Some(&before), &after).cpu_percent, Some(20));
    }

    #[test]
    fn readings_come_at_the_interval_after_a_quick_second_one() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let config = crate::config::SystemUsageConfig {
            interval_seconds: 5,
            popup: None,
        };
        app.configure_system_usage(Some(&config));
        let start = app.next_system_usage_deadline().expect("due at once");
        app.refresh_system_usage(start);
        if crate::platform::system_usage_sample().is_some() {
            let second = app.next_system_usage_deadline().expect("due");
            assert_eq!(second - start, FIRST_CPU_FIGURE_AFTER);
            app.refresh_system_usage(second);
            assert_eq!(
                app.next_system_usage_deadline().expect("due") - second,
                Duration::from_secs(5)
            );
            // Its CPU figure needs clock ticks to pass between the readings, which a test's
            // two back to back may not see.
            assert!(app
                .system_usage()
                .is_some_and(|usage| usage.memory_total_bytes > 0));
        }
        assert!(
            !app.refresh_system_usage(start),
            "nothing to do before it's due"
        );

        app.configure_system_usage(Some(&crate::config::SystemUsageConfig {
            interval_seconds: 0,
            popup: None,
        }));
        assert_eq!(app.next_system_usage_deadline(), None, "off with a problem");
        assert_eq!(app.system_usage(), None);
    }
}

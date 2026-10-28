//! Reporting the GPU priority of the window's GL context (ADR-0075).

use slam_input::{ContextPriority, PriorityOutcome};

/// Writes how the context's GPU priority was set up to the log.
pub fn log_priority(outcome: &PriorityOutcome) {
    const HINT: &str = "grant it with `sudo setcap cap_sys_nice+ep <path to slam>`";
    match outcome {
        PriorityOutcome::Granted(ContextPriority::High) => {
            tracing::info!("gpu: high-priority context");
        }
        PriorityOutcome::Granted(level) => tracing::info!("gpu: context priority {level:?}"),
        PriorityOutcome::Unsupported => {
            tracing::info!("gpu: context priority is not supported, using the default");
        }
        PriorityOutcome::NoPermission => tracing::warn!(
            "gpu: no CAP_SYS_NICE, frames share the GPU queue with other programs; {HINT}"
        ),
        PriorityOutcome::Refused(err) => {
            tracing::warn!(
                "gpu: high-priority context failed ({err}), using the default; \
                 if the driver needs CAP_SYS_NICE, {HINT}"
            );
        }
    }
}

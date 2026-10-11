//! Headless replay playback: replay frames as input and the simulator that steps the rules
//! through them (ADR-0028, ADR-0029).

pub mod input;
pub mod sim;

pub use input::{
    ActionEvent, Actions, InputFrame, InputState, MAX_STEP_EVENTS, ReplayCursor, ReplayInput,
};
pub use sim::{NoRules, Rules, Simulator, StepInput, UPDATE_INTERVAL, gameplay_start_time};

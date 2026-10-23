//! Headless replay playback: replay frames as input and the simulator that steps the rules
//! through them (ADR-0028, ADR-0029).

pub mod export;
pub mod input;
pub mod record;
pub mod sim;

pub use export::{LAZER_REPLAY_VERSION, PlayScore, ReplayInfo, legacy_buttons, replay_of_play};
pub use input::{
    ActionEvent, Actions, InputFrame, InputState, MAX_STEP_EVENTS, ReplayCursor, ReplayInput,
};
pub use record::{RECORD_FRAME_RATE, ReplayRecorder};
pub use sim::{NoRules, Rules, Simulator, StepInput, UPDATE_INTERVAL};

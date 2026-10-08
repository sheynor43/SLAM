#![allow(dead_code)]

use sdl3_sys::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_KEY_UP, SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP,
    SDL_EVENT_MOUSE_MOTION, SDL_Event, SDL_KeyboardEvent, SDL_MouseButtonEvent,
    SDL_MouseMotionEvent,
};
use sdl3_sys::scancode::SDL_Scancode;

pub fn key(timestamp: u64, scancode: SDL_Scancode, down: bool, repeat: bool) -> SDL_Event {
    SDL_Event {
        key: SDL_KeyboardEvent {
            r#type: if down {
                SDL_EVENT_KEY_DOWN
            } else {
                SDL_EVENT_KEY_UP
            },
            timestamp,
            scancode,
            down,
            repeat,
            ..Default::default()
        },
    }
}

pub fn button(timestamp: u64, index: u8, down: bool) -> SDL_Event {
    SDL_Event {
        button: SDL_MouseButtonEvent {
            r#type: if down {
                SDL_EVENT_MOUSE_BUTTON_DOWN
            } else {
                SDL_EVENT_MOUSE_BUTTON_UP
            },
            timestamp,
            button: index,
            down,
            ..Default::default()
        },
    }
}

pub fn motion(timestamp: u64, x: f32, y: f32) -> SDL_Event {
    SDL_Event {
        motion: SDL_MouseMotionEvent {
            r#type: SDL_EVENT_MOUSE_MOTION,
            timestamp,
            x,
            y,
            ..Default::default()
        },
    }
}

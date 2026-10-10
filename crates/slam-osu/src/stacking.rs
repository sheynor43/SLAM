//! Stacking: objects placed on top of each other in quick succession are shifted apart.

use crate::objects::{OsuHitObject, OsuHitObjectKind};

/// Lazer's `OsuBeatmapProcessor.STACK_DISTANCE`: the maximum distance between the end of one
/// object and the start of another which allows the objects to be stacked on top of another.
pub const STACK_DISTANCE: f32 = 3.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (ApplyStacking)
/// Recomputes the stack heights of all objects (and their nested objects) with the algorithm of
/// the beatmap's format version. Objects must be sorted by start time and have their defaults
/// applied (stacking reads the preempt and slider end times).
pub(crate) fn apply_stacking(
    objects: &mut [OsuHitObject],
    format_version: i32,
    stack_leniency: f32,
) {
    if objects.is_empty() {
        return;
    }

    // Reset stacking
    for h in objects.iter_mut() {
        h.stack_height = 0;
    }

    if format_version >= 6 {
        apply_stacking_new(objects, stack_leniency);
    } else {
        apply_stacking_old(objects, stack_leniency);
    }

    // Lazer pushes every change of `StackHeight` to the nested objects through a bindable;
    // only the final value is visible.
    for h in objects.iter_mut() {
        let height = h.stack_height;
        h.set_stack_height(height);
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (applyStacking)
/// Stacking for format version 6 and later, over the whole map (`startIndex = 0`,
/// `endIndex = Count - 1`). With the full range lazer's extension of the end index and the
/// resets of objects outside the range never happen, so they are left out.
fn apply_stacking_new(objects: &mut [OsuHitObject], stack_leniency: f32) {
    // Reverse pass for stack calculation.
    for i in (1..objects.len()).rev() {
        let mut n = i;
        // We should check every note which has not yet got a stack. Consider the case we have
        // two interwound stacks and this will make sense.
        //
        // o <-1      o <-2
        //  o <-3      o <-4
        //
        // We first process starting from 4 and handle 2, then we come backwards on the i loop
        // iteration until we reach 3 and handle 1. 2 and 1 will be ignored in the i loop
        // because they already have a stack value.

        let mut object_i = i;
        if objects[i].stack_height != 0 || objects[i].is_spinner() {
            continue;
        }

        let stack_threshold = calculate_stack_threshold(&objects[i], stack_leniency);

        match objects[i].kind {
            // If this object is a hitcircle, then we enter this "special" case. It either ends
            // with a stack of hitcircles only, or a stack of hitcircles that are underneath a
            // slider. Any other case is handled by the slider branch below.
            OsuHitObjectKind::Circle => {
                while n > 0 {
                    n -= 1;
                    if objects[n].is_spinner() {
                        continue;
                    }

                    let end_time = objects[n].end_time();

                    // Truncation to integer is required to match stable: both quantities being
                    // subtracted there are integers. C# compares the int difference with the
                    // float threshold as floats, and int subtraction wraps.
                    let difference =
                        (objects[object_i].start_time as i32).wrapping_sub(end_time as i32);
                    if difference as f32 > stack_threshold {
                        // We are no longer within stacking range of the previous object.
                        break;
                    }

                    // This is a special case where hitcircles are moved DOWN and RIGHT
                    // (negative stacking) if they are under the *last* slider in a stacked
                    // pattern.
                    //    o==o <- slider is at original location
                    //        o <- hitCircle has stack of -1
                    //         o <- hitCircle has stack of -2
                    let n_end_position = objects[n].end_position();
                    if matches!(objects[n].kind, OsuHitObjectKind::Slider(_))
                        && n_end_position.distance(objects[object_i].position) < STACK_DISTANCE
                    {
                        let offset = objects[object_i].stack_height - objects[n].stack_height + 1;

                        for object_j in &mut objects[n + 1..=i] {
                            // For each object which was declared under this slider, we will
                            // offset it to appear *below* the slider end (rather than above).
                            if n_end_position.distance(object_j.position) < STACK_DISTANCE {
                                object_j.stack_height -= offset;
                            }
                        }

                        // We have hit a slider. We should restart calculation using this as
                        // the new base. Breaking here will mean that the slider still has a
                        // stack height of 0, so will be handled in the i-outer-loop.
                        break;
                    }

                    if objects[n].position.distance(objects[object_i].position) < STACK_DISTANCE {
                        // Keep processing as if there are no sliders. If we come across a
                        // slider, this gets cancelled out. Sliders with start positions
                        // stacking are a special case that is also handled here.
                        objects[n].stack_height = objects[object_i].stack_height + 1;
                        object_i = n;
                    }
                }
            }
            OsuHitObjectKind::Slider(_) => {
                // We have hit the first slider in a possible stack. From this point on, we
                // ALWAYS stack positive regardless.
                while n > 0 {
                    n -= 1;
                    if objects[n].is_spinner() {
                        continue;
                    }

                    if objects[object_i].start_time - objects[n].start_time
                        > f64::from(stack_threshold)
                    {
                        // We are no longer within stacking range of the previous object.
                        break;
                    }

                    if objects[n]
                        .end_position()
                        .distance(objects[object_i].position)
                        < STACK_DISTANCE
                    {
                        objects[n].stack_height = objects[object_i].stack_height + 1;
                        object_i = n;
                    }
                }
            }
            OsuHitObjectKind::Spinner { .. } => {}
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (applyStackingOld)
/// Stacking for format versions before 6.
fn apply_stacking_old(objects: &mut [OsuHitObject], stack_leniency: f32) {
    for i in 0..objects.len() {
        let is_slider = matches!(objects[i].kind, OsuHitObjectKind::Slider(_));
        if objects[i].stack_height != 0 && !is_slider {
            continue;
        }

        let mut start_time = objects[i].end_time();
        let mut slider_stack = 0;

        for j in i + 1..objects.len() {
            let stack_threshold = calculate_stack_threshold(&objects[i], stack_leniency);

            if objects[j].start_time - f64::from(stack_threshold) > start_time {
                break;
            }

            // The start position of the hitobject, or the position at the end of the path if
            // the hitobject is a slider (the end of the first span, not of the slider).
            let position2 = match &objects[i].kind {
                OsuHitObjectKind::Slider(slider) => {
                    objects[i].position + slider.path.position_at(1.0)
                }
                _ => objects[i].position,
            };

            // The use of the start time of `j` below doesn't match stable's use of its end
            // time: stable never computes it for the inner-loop object, so it equals the start
            // time there.
            if objects[j].position.distance(objects[i].position) < STACK_DISTANCE {
                objects[i].stack_height += 1;
                start_time = objects[j].start_time;
            } else if objects[j].position.distance(position2) < STACK_DISTANCE {
                // Case for sliders - bump notes down and right, rather than up and left.
                slider_stack += 1;
                objects[j].stack_height -= slider_stack;
                start_time = objects[j].start_time;
            }
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapProcessor.cs (calculateStackThreshold)
/// The preempt truncated to an integer and the float result are both stable's.
fn calculate_stack_threshold(object: &OsuHitObject, stack_leniency: f32) -> f32 {
    // C#'s (int) cast saturates like `as` since .NET 9; the int is converted to float.
    (object.defaults.time_preempt as i32) as f32 * stack_leniency
}

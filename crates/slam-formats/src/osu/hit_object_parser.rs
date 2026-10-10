//! Port of osu!lazer's `ConvertHitObjectParser`.

use super::decode::{WarningKind, field, num};
use super::hit_object::{
    HitObject, HitObjectKind, HitSample, HitSampleName, PathControlPoint, PathType, SampleBank,
    Slider, Vec2,
};
use super::parsing::{self, MAX_COORDINATE_VALUE};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapEncoder.cs
const FIRST_LAZER_VERSION: i32 = 128;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyHitObjectType.cs
const TYPE_CIRCLE: i32 = 1;
const TYPE_SLIDER: i32 = 1 << 1;
const TYPE_NEW_COMBO: i32 = 1 << 2;
const TYPE_SPINNER: i32 = 1 << 3;
const TYPE_COMBO_OFFSET: i32 = (1 << 4) | (1 << 5) | (1 << 6);
const TYPE_HOLD: i32 = 1 << 7;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Legacy/LegacyHitSoundType.cs
const SOUND_NORMAL: i32 = 1;
const SOUND_WHISTLE: i32 = 2;
const SOUND_FINISH: i32 = 4;
const SOUND_CLAP: i32 = 8;

/// Slider repeat counts above this are rejected.
const MAX_REPEAT_COUNT: i32 = 9000;

/// Lazer's private `SampleBankInfo`.
#[derive(Clone)]
struct SampleBankInfo {
    filename: Option<String>,
    bank_for_normal: Option<SampleBank>,
    bank_for_additions: Option<SampleBank>,
    volume: i32,
    custom_sample_bank: i32,
    editor_auto_bank: bool,
}

impl SampleBankInfo {
    fn new() -> Self {
        Self {
            filename: None,
            bank_for_normal: None,
            bank_for_additions: None,
            volume: 0,
            custom_sample_bank: 0,
            editor_auto_bank: true,
        }
    }
}

/// Stateful per-file hit object parser; lines must be fed in file order.
pub(super) struct HitObjectParser {
    offset: f64,
    format_version: i32,
    first_object: bool,
    /// Whether lazer's `lastObject` is a `ConvertSpinner`.
    last_is_spinner: bool,
}

impl HitObjectParser {
    pub(super) fn new(offset: f64, format_version: i32) -> Self {
        Self {
            offset,
            format_version,
            first_object: true,
            last_is_spinner: false,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    fn read_vec(&self, x: &str, y: &str) -> Result<Vec2, WarningKind> {
        let limit = MAX_COORDINATE_VALUE as f32;
        let x = num(parsing::parse_float(x, limit, false))?;
        let y = num(parsing::parse_float(y, limit, false))?;
        Ok(if self.format_version >= FIRST_LAZER_VERSION {
            Vec2::new(x, y)
        } else {
            // `(int)` truncation of the legacy format.
            Vec2::new((x as i32) as f32, (y as i32) as f32)
        })
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    pub(super) fn parse(&mut self, text: &str) -> Result<HitObject, WarningKind> {
        let split: Vec<&str> = text.split(',').collect();

        let pos = self.read_vec(field(&split, 0)?, field(&split, 1)?)?;
        let start_time = num(parsing::double(field(&split, 2)?))? + self.offset;

        let mut ty = num(parsing::int(field(&split, 3)?))?;
        let combo_offset = (ty & TYPE_COMBO_OFFSET) >> 4;
        ty &= !TYPE_COMBO_OFFSET;
        let combo = ty & TYPE_NEW_COMBO != 0;
        ty &= !TYPE_NEW_COMBO;

        let sound_type = num(parsing::int(field(&split, 4)?))?;
        let mut bank_info = SampleBankInfo::new();

        let mut new_combo = false;
        let mut object_combo_offset = 0;
        let mut samples = Vec::new();
        let kind;
        let mut position = pos;

        if ty & TYPE_CIRCLE != 0 {
            (new_combo, object_combo_offset) = self.combo_state(combo, combo_offset);
            // `lastObject` is assigned before the sample banks are read.
            self.last_is_spinner = false;
            kind = HitObjectKind::Circle;

            if split.len() > 5 {
                read_custom_sample_banks(split[5], &mut bank_info, false)?;
            }
        } else if ty & TYPE_SLIDER != 0 {
            let mut length: Option<f64> = None;

            let mut repeat_count = num(parsing::int(field(&split, 6)?))?;
            if repeat_count > MAX_REPEAT_COUNT {
                return Err(WarningKind::NumberOutOfRange);
            }
            // osu-stable treated the first span of the slider as a repeat.
            repeat_count = repeat_count.saturating_sub(1).max(0);

            if split.len() > 7 {
                let l = num(parsing::parse_double(
                    split[7],
                    f64::from(MAX_COORDINATE_VALUE),
                    false,
                ))?
                .max(0.0);
                length = if l == 0.0 { None } else { Some(l) };
            }

            if split.len() > 10 {
                read_custom_sample_banks(split[10], &mut bank_info, true)?;
            }

            // One node for each repeat plus the start and end nodes.
            let nodes = repeat_count as usize + 2;

            let mut node_bank_infos = vec![bank_info.clone(); nodes];
            if split.len() > 9 && !split[9].is_empty() {
                for (info, set) in node_bank_infos.iter_mut().zip(split[9].split('|')) {
                    read_custom_sample_banks(set, info, false)?;
                }
            }

            let mut node_sound_types = vec![sound_type; nodes];
            if split.len() > 8 && !split[8].is_empty() {
                for (st, add) in node_sound_types.iter_mut().zip(split[8].split('|')) {
                    // `int.TryParse` leaves 0 on failure.
                    *st = parsing::net_parse_i32(add).unwrap_or(0);
                }
            }

            let node_samples: Vec<Vec<HitSample>> = node_sound_types
                .iter()
                .zip(&node_bank_infos)
                .map(|(&st, info)| convert_sound_type(st, info))
                .collect();

            let control_points = self.convert_path_string(field(&split, 5)?, pos)?;

            (new_combo, object_combo_offset) = self.combo_state(combo, combo_offset);
            self.last_is_spinner = false;
            kind = HitObjectKind::Slider(Slider {
                control_points,
                expected_distance: length,
                repeat_count,
                node_samples,
            });
        } else if ty & TYPE_SPINNER != 0 {
            let duration =
                (num(parsing::double(field(&split, 5)?))? + self.offset - start_time).max(0.0);

            position = Vec2::new(256.0, 192.0);
            // Spinners ignore the first-object / after-spinner rule and have no combo offset.
            new_combo = combo;
            self.last_is_spinner = true;
            kind = HitObjectKind::Spinner { duration };

            if split.len() > 6 {
                read_custom_sample_banks(split[6], &mut bank_info, false)?;
            }
        } else if ty & TYPE_HOLD != 0 {
            // Hold is generated by BMS converts.
            let mut end_time = start_time.max(num(parsing::double(field(&split, 2)?))?);

            if split.len() > 5 && !split[5].is_empty() {
                let mut ss = split[5].split(':');
                end_time = start_time.max(num(parsing::double(ss.next().unwrap_or("")))?);
                let rest: Vec<&str> = ss.collect();
                read_custom_sample_banks(&rest.join(":"), &mut bank_info, false)?;
            }

            self.last_is_spinner = false;
            kind = HitObjectKind::Hold {
                duration: end_time + self.offset - start_time,
            };
        } else {
            return Err(WarningKind::UnknownHitObjectType);
        }

        if samples.is_empty() {
            samples = convert_sound_type(sound_type, &bank_info);
        }

        self.first_object = false;

        Ok(HitObject {
            start_time,
            position,
            new_combo,
            combo_offset: object_combo_offset,
            legacy_type: ty,
            samples,
            kind,
        })
    }

    /// `NewCombo = firstObject || lastObject is ConvertSpinner || newCombo`,
    /// `ComboOffset = newCombo ? comboOffset : 0`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    fn combo_state(&self, combo: bool, combo_offset: i32) -> (bool, i32) {
        (
            self.first_object || self.last_is_spinner || combo,
            if combo { combo_offset } else { 0 },
        )
    }

    /// Converts a point string such as `B|1:1|2:2|2:2|3:3|L|1:1` into control points.
    ///
    /// Divergence from lazer, on malformed input only: "letter" is `char::is_alphabetic` on a
    /// BMP character, which is not identical to .NET `char.IsLetter` (it also accepts letter
    /// numbers such as U+2160 and other-alphabetic marks such as U+0345).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    fn convert_path_string(
        &self,
        point_string: &str,
        offset: Vec2,
    ) -> Result<Vec<PathControlPoint>, WarningKind> {
        let mut points: Vec<Vec2> = Vec::new();
        let mut segments: Vec<(PathType, usize)> = Vec::new();

        for s in point_string.split('|') {
            let first = s.chars().next().ok_or(WarningKind::MissingField)?;
            if first.len_utf16() == 1 && first.is_alphabetic() {
                segments.push((convert_path_type(s), points.len()));
                // The first segment is prepended by an extra zero point.
                if points.is_empty() {
                    points.push(Vec2::ZERO);
                }
            } else {
                points.push(self.read_point(s, offset)?);
            }
        }

        let mut control_points = Vec::new();
        for (i, &(ty, start)) in segments.iter().enumerate() {
            if let Some(&(_, end)) = segments.get(i + 1) {
                // `end == points.len()` means the next segment is empty, which fails below.
                let end_point = points.get(end).copied().unwrap_or(Vec2::ZERO);
                self.convert_points(
                    ty,
                    &points[start..end],
                    Some(end_point),
                    &mut control_points,
                )?;
            } else {
                self.convert_points(ty, &points[start..], None, &mut control_points)?;
            }
        }
        Ok(control_points)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    fn read_point(&self, value: &str, start_pos: Vec2) -> Result<Vec2, WarningKind> {
        let mut it = value.split(':');
        let x = it.next().unwrap_or("");
        let y = it.next().ok_or(WarningKind::MissingField)?;
        let p = self.read_vec(x, y)?;
        Ok(Vec2::new(p.x - start_pos.x, p.y - start_pos.y))
    }

    /// Converts a point list of one explicit segment into control points, splitting it into
    /// implicit segments at duplicate points. The shared duplicate points are not returned.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
    fn convert_points(
        &self,
        mut ty: PathType,
        points: &[Vec2],
        end_point: Option<Vec2>,
        out: &mut Vec<PathControlPoint>,
    ) -> Result<(), WarningKind> {
        let mut vertices: Vec<PathControlPoint> = points
            .iter()
            .map(|&position| PathControlPoint {
                position,
                path_type: None,
            })
            .collect();

        // Edge-case rules (to match stable).
        if ty == PathType::PerfectCurve {
            let end_point_length = usize::from(end_point.is_some());

            if self.format_version < FIRST_LAZER_VERSION {
                if vertices.len() + end_point_length != 3 {
                    ty = PathType::Bezier;
                } else {
                    let third = end_point.or_else(|| points.get(2).copied());
                    if let (Some(p0), Some(p1), Some(p2)) = (points.first(), points.get(1), third)
                        && is_linear(*p0, *p1, p2)
                    {
                        // osu-stable special-cased colinear perfect curves to a linear path.
                        ty = PathType::Linear;
                    }
                }
            } else if vertices.len() + end_point_length > 3 {
                // Lazer supports perfect curves with fewer than 3 points and colinear points.
                ty = PathType::Bezier;
            }
        }

        // The first control point must have a definite type (an empty segment throws in lazer).
        vertices
            .first_mut()
            .ok_or(WarningKind::MissingField)?
            .path_type = Some(ty);

        let mut start_index = 0;
        let mut end_index = 0;

        loop {
            end_index += 1;
            if end_index >= vertices.len() {
                break;
            }

            // Keep incrementing while an implicit segment doesn't need to be started.
            if vertices[end_index].position != vertices[end_index - 1].position {
                continue;
            }

            // Legacy CATMULL sliders don't support multiple segments.
            if ty == PathType::Catmull && end_index > 1 && self.format_version < FIRST_LAZER_VERSION
            {
                continue;
            }

            // The last control point of each segment is not allowed to start a new implicit segment.
            if end_index == vertices.len() - 1 {
                continue;
            }

            // Force a type on the last point, and return the current control point set as a segment.
            vertices[end_index - 1].path_type = Some(ty);
            out.extend_from_slice(&vertices[start_index..end_index]);

            // Skip the current control point: it is the same as the one just returned.
            start_index = end_index + 1;
        }

        if start_index < end_index {
            out.extend_from_slice(&vertices[start_index..end_index]);
        }
        Ok(())
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
fn is_linear(p0: Vec2, p1: Vec2, p2: Vec2) -> bool {
    // Precision.AlmostEquals(0, v) with FLOAT_EPSILON = 1e-3f
    let v = (p1.y - p0.y) * (p2.x - p0.x) - (p1.x - p0.x) * (p2.y - p0.y);
    (0.0f32 - v).abs() <= 1e-3
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
fn convert_path_type(input: &str) -> PathType {
    match input.as_bytes()[0] {
        b'B' => match input.get(1..).filter(|s| !s.is_empty()) {
            Some(rest) => match parsing::net_parse_i32(rest) {
                Ok(degree) if degree > 0 => PathType::BSpline { degree },
                _ => PathType::Bezier,
            },
            None => PathType::Bezier,
        },
        b'L' => PathType::Linear,
        b'P' => PathType::PerfectCurve,
        // 'C' and anything else.
        _ => PathType::Catmull,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
fn read_custom_sample_banks(
    s: &str,
    bank_info: &mut SampleBankInfo,
    banks_only: bool,
) -> Result<(), WarningKind> {
    if s.is_empty() {
        return Ok(());
    }

    let split: Vec<&str> = s.split(':').collect();

    // 0 none, 1 normal, 2 soft, 3 drum; undefined values become normal.
    let bank = sample_bank(num(parsing::int(field(&split, 0)?))?);
    let add_bank = sample_bank(num(parsing::int(field(&split, 1)?))?);

    let string_bank = bank;
    let string_add_bank = add_bank;

    bank_info.editor_auto_bank = string_add_bank.is_none();
    bank_info.bank_for_normal = string_bank;
    bank_info.bank_for_additions = string_add_bank.or(string_bank);

    if banks_only {
        return Ok(());
    }

    if split.len() > 2 {
        bank_info.custom_sample_bank = num(parsing::int(split[2]))?;
    }
    if split.len() > 3 {
        bank_info.volume = num(parsing::int(split[3]))?.max(0);
    }
    bank_info.filename = split.get(4).map(|s| (*s).to_owned());
    Ok(())
}

/// `(LegacySampleBank)value` with `Enum.IsDefined` fallback to normal; `None` is "none".
fn sample_bank(value: i32) -> Option<SampleBank> {
    match value {
        0 => None,
        2 => Some(SampleBank::Soft),
        3 => Some(SampleBank::Drum),
        _ => Some(SampleBank::Normal),
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
fn legacy_sample(
    name: HitSampleName,
    bank: Option<SampleBank>,
    volume: i32,
    editor_auto_bank: bool,
    custom_sample_bank: i32,
    is_layered: bool,
) -> HitSample {
    HitSample {
        name,
        bank: bank.unwrap_or(SampleBank::Normal),
        bank_specified: bank.is_some(),
        suffix: (custom_sample_bank >= 2).then_some(custom_sample_bank),
        volume,
        editor_auto_bank,
        use_beatmap_samples: custom_sample_bank >= 1,
        is_layered,
        filename: None,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs
fn convert_sound_type(ty: i32, info: &SampleBankInfo) -> Vec<HitSample> {
    let mut samples = Vec::with_capacity(4);

    match info.filename.as_deref().filter(|f| !f.is_empty()) {
        None => samples.push(legacy_sample(
            HitSampleName::Normal,
            info.bank_for_normal,
            info.volume,
            true,
            info.custom_sample_bank,
            // A sound type without the Normal flag still attaches a layered normal sample;
            // `None` counts as a normal, non-layered sample.
            ty != 0 && ty & SOUND_NORMAL == 0,
        )),
        Some(filename) => {
            // FileHitSampleInfo: normal bank, custom sample bank 1, not editor-auto.
            let mut s = legacy_sample(
                HitSampleName::Normal,
                Some(SampleBank::Normal),
                info.volume,
                false,
                1,
                false,
            );
            s.filename = Some(filename.to_owned());
            samples.push(s);
        }
    }

    for (flag, name) in [
        (SOUND_FINISH, HitSampleName::Finish),
        (SOUND_WHISTLE, HitSampleName::Whistle),
        (SOUND_CLAP, HitSampleName::Clap),
    ] {
        if ty & flag != 0 {
            samples.push(legacy_sample(
                name,
                info.bank_for_additions,
                info.volume,
                info.editor_auto_bank,
                info.custom_sample_bank,
                false,
            ));
        }
    }

    samples
}

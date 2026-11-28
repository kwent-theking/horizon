//! Client board layout from Create 6.0.10's MIT-licensed `FlapDisplayBlockEntity`,
//! `FlapDisplaySection` and `FlapDisplayRenderer`.

use super::indicators::create_dye_colors;
use super::machines::{centred, horizontal_angle};
use glam::{Mat4, Quat, Vec3};
use horizon_client::block_entity_render::TextCall;
use horizon_client::world::{
    FlapDisplayRow, FlapDisplaySection, FlapDisplayState, FlapDisplayTransition,
};
use horizon_client_core::component::to_text;
use horizon_client_core::lang;
use horizon_client_core::nbt_value::{float, number, text};
use horizon_client_core::text::Text;
use horizon_world::nbt::Tag;

const PIXELS_PER_BLOCK: f32 = 32.0;
const FLAP_ADVANCE: f32 = 7.0;
const WIDE_FLAP_ADVANCE: f32 = 9.0;
const DEFAULT_COLOR: u32 = 0xD3C6BA;
const BOARD_FACE: f32 = 53.0 / 64.0;
const TRANSITION_TICKS: f64 = 8.0;
const DEFAULT_MEDIUM_SPEED: f32 = 30.0;

fn cycle_options(tag: &Tag) -> Vec<Vec<char>> {
    if let Some(cycle) = text(tag.get("Cycle")) {
        let key = format!("create.flap_display.cycles.{cycle}");
        let options = lang::get(&key);
        if options != key {
            let options: Vec<_> = options
                .split(';')
                .filter(|part| !part.is_empty())
                .map(|part| part.chars().collect())
                .collect();
            if !options.is_empty() {
                return options;
            }
        }
    }
    vec![vec!['-'], vec!['_']]
}

fn read_section(
    tag: &Tag,
    previous: Option<&FlapDisplaySection>,
    board_pixels: f32,
    tick: i64,
) -> Option<FlapDisplaySection> {
    let width = float(tag.get("Width"))? as f32;
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    let width = width.min(board_pixels);
    let single = tag.get("SingleFlap").is_some();
    let wide = tag.get("Wide").is_some();
    let right_aligned = tag.get("RightAligned").is_some();
    let limit = if single {
        width.ceil()
    } else {
        (width / FLAP_ADVANCE).floor()
    } as usize;
    let decoded = text(tag.get("Text"))
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .filter(|value| !value.is_null())
        .map(|value| to_text(&value, &mut |_| Text::empty()).plain());
    let characters: Vec<char> = if let Some(decoded) = decoded {
        if single {
            decoded.chars().take(limit).collect()
        } else {
            let line = if right_aligned {
                decoded.trim()
            } else {
                &decoded
            };
            let mut cells: Vec<_> = line
                .chars()
                .flat_map(char::to_uppercase)
                .take(limit)
                .collect();
            let padding = limit - cells.len();
            cells.resize(limit, ' ');
            if right_aligned {
                cells.rotate_right(padding);
            }
            cells
        }
    } else {
        previous.map_or_else(Vec::new, |section| {
            section.characters.iter().copied().take(limit).collect()
        })
    };
    let transition = match previous {
        Some(old) if old.characters == characters => old
            .transition
            .as_ref()
            .filter(|active| {
                (0..TRANSITION_TICKS as i64).contains(&tick.saturating_sub(active.since))
            })
            .cloned(),
        _ if number(tag.get("Transition")).is_some_and(|marker| marker != 0) => {
            Some(FlapDisplayTransition {
                since: tick,
                previous: previous.map_or_else(Vec::new, |section| section.characters.clone()),
                options: cycle_options(tag),
            })
        }
        _ => None,
    };
    Some(FlapDisplaySection {
        width,
        gap: if tag.get("Gap").is_some() { 8.0 } else { 1.0 },
        cell_width: if single {
            0.0
        } else if wide {
            WIDE_FLAP_ADVANCE
        } else {
            FLAP_ADVANCE
        },
        characters,
        transition,
    })
}

fn read_sections(
    section_tags: &[Tag],
    previous: Option<&FlapDisplayRow>,
    board_pixels: f32,
    tick: i64,
) -> Vec<FlapDisplaySection> {
    // Allow an oversized layout to be clipped, while bounding the decoded text by the board.
    let mut remaining = 2.0 * board_pixels;
    let mut sections = Vec::new();
    for (index, tag) in section_tags.iter().enumerate() {
        if remaining <= 0.0 {
            break;
        }
        let old = previous.and_then(|row| row.sections.get(index));
        if let Some(section) = read_section(tag, old, board_pixels.min(remaining), tick) {
            remaining -= section.width + section.gap;
            sections.push(section);
        }
    }
    sections
}

pub(super) fn apply_flap_display_tag(state: &mut FlapDisplayState, tag: &Tag, tick: i64) -> bool {
    let Some(controller) = number(tag.get("Controller")) else {
        return state.width != 0;
    };
    if controller == 0 {
        *state = FlapDisplayState::default();
        return false;
    }
    // Create scans at most 32 blocks across and checks 32 downward connections.
    let width = number(tag.get("XSize")).unwrap_or(1).clamp(1, 32) as u8;
    let height = number(tag.get("YSize")).unwrap_or(1).clamp(1, 33) as u8;
    let board_pixels = f32::from(width) * PIXELS_PER_BLOCK;
    let old_rows = if width == state.width && height == state.height {
        std::mem::take(&mut state.rows)
    } else {
        Vec::new()
    };
    state.width = width;
    state.height = height;
    state.rows = (0..usize::from(height) * 2)
        .map(|row| {
            let color = text(tag.get(&format!("Dye{row}")))
                .and_then(|dye| create_dye_colors(&dye.to_ascii_lowercase()))
                .map_or(DEFAULT_COLOR, |(bright, _)| bright);
            let glowing = tag.get(&format!("GlowingLine{row}")).is_some();
            let layout = tag.get(&format!("Display{row}"));
            let key = text(layout.and_then(|layout| layout.get("Key"))).unwrap_or_default();
            let previous = old_rows.get(row).filter(|old| old.key == key);
            let sections = match layout.and_then(|layout| layout.get("Sections")) {
                Some(Tag::List(sections)) => read_sections(sections, previous, board_pixels, tick),
                _ => Vec::new(),
            };
            FlapDisplayRow {
                key,
                color,
                glowing,
                sections,
            }
        })
        .collect();
    true
}

fn board_rotation(facing: &str) -> Mat4 {
    centred(Quat::from_rotation_y(horizontal_angle(facing).to_radians()))
}

/// Controller-relative bounds include the full board even when it crosses chunks.
pub(super) fn flap_display_bounds(state: &FlapDisplayState, facing: &str) -> (Vec3, Vec3) {
    let rotation = board_rotation(facing);
    let lower = rotation.transform_point3(Vec3::new(0.0, 1.0 - f32::from(state.height), 0.0));
    let upper = rotation.transform_point3(Vec3::new(f32::from(state.width), 1.0, 1.0));
    (lower.min(upper), lower.max(upper))
}

fn active_transition(
    section: &FlapDisplaySection,
    time: f64,
    speed: f32,
) -> Option<(&FlapDisplayTransition, usize)> {
    let transition = section.transition.as_ref()?;
    let age = time - transition.since as f64;
    ((DEFAULT_MEDIUM_SPEED..=128.0).contains(&speed.abs())
        && (0.0..TRANSITION_TICKS).contains(&age))
    .then_some((transition, (age / 2.0) as usize))
}

pub(super) fn flap_display_text_calls(
    state: &FlapDisplayState,
    facing: &str,
    at: Vec3,
    light: u32,
    time: f64,
    speed: f32,
    glyph_advance: &dyn Fn(char) -> f32,
) -> Vec<TextCall> {
    let matrix = Mat4::from_translation(at)
        * board_rotation(facing)
        * Mat4::from_translation(Vec3::new(0.0, 1.0, BOARD_FACE))
        * Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0) / PIXELS_PER_BLOCK);
    let board_pixels = f32::from(state.width) * PIXELS_PER_BLOCK;
    let mut calls = Vec::new();
    for (index, row) in state.rows.iter().enumerate() {
        let layout_pixels: f32 = row
            .sections
            .iter()
            .map(|section| section.width + section.gap)
            .sum();
        let mut section_x = (board_pixels - layout_pixels) / 2.0 + 1.0;
        let y = 4.5 + 16.0 * index as f32;
        for section in &row.sections {
            let start = section_x.max(0.0);
            let end = (section_x + section.width).min(board_pixels);
            let transition = active_transition(section, time, speed);
            let proportional = section.cell_width == 0.0;
            let mut cursor = section_x;
            let characters = match transition {
                Some((animation, frame)) if proportional => {
                    &animation.options[frame % animation.options.len()]
                }
                _ => &section.characters,
            };
            for (cell, &ch) in characters.iter().enumerate() {
                let ch = match transition {
                    Some((animation, frame))
                        if !proportional
                            && animation.previous.get(cell).copied().unwrap_or(' ') != ch =>
                    {
                        animation.options[(frame + cell * 7) % animation.options.len()][0]
                    }
                    _ => ch,
                };
                let advance = glyph_advance(ch);
                let x = if proportional {
                    cursor
                } else {
                    cursor + (section.cell_width - advance) / 2.0
                };
                cursor += if proportional {
                    advance
                } else {
                    section.cell_width
                };
                if !ch.is_whitespace() && x >= start && x + advance <= end {
                    calls.push(TextCall {
                        matrix,
                        x,
                        y,
                        text: ch.to_string(),
                        color: 0xFF00_0000 | row.color,
                        outline: 0,
                        light: if row.glowing { 0xF0F0 } else { light },
                    });
                }
            }
            section_x += section.width + section.gap;
        }
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::parse_snbt;

    fn tag(s: &str) -> Tag {
        parse_snbt(s).unwrap().0
    }

    fn calls(state: &FlapDisplayState, facing: &str, time: f64, speed: f32) -> Vec<TextCall> {
        flap_display_text_calls(state, facing, Vec3::ZERO, 0x4050, time, speed, &|_| 6.0)
    }

    fn position(call: &TextCall) -> Vec3 {
        call.matrix.transform_point3(Vec3::new(call.x, call.y, 0.0))
    }

    #[test]
    fn flap_display_reads_controller_rows_and_centres_sections() {
        let mut state = FlapDisplayState::default();
        let packet = tag(
            r#"{Controller:1b,XSize:2,YSize:1,GlowingLine0:1b,Dye0:"YELLOW",Display0:{Key:"Departure",Sections:[{Width:35f,Cycle:"alphabet",Gap:1b,Text:'{"text":"north"}'},{Width:14f,Cycle:"numeric",RightAligned:1b,Text:'{"text":"7"}'}]},Display1:{Key:"Default",Sections:[{Width:56f,Cycle:"alphabet",Text:'{"text":"Track 2"}'}]}}"#,
        );
        assert!(apply_flap_display_tag(&mut state, &packet, 5));
        let display = calls(&state, "south", 6.0, 0.0);
        assert_eq!(
            display
                .iter()
                .map(|call| call.text.as_str())
                .collect::<String>(),
            "NORTH7TRACK2"
        );
        assert!(
            display[..6]
                .iter()
                .all(|call| call.light == 0xF0F0 && call.color == 0xFFE6D756)
        );
        assert!(display[6..].iter().all(|call| call.light == 0x4050));
        assert!(
            position(&display[0])
                .abs_diff_eq(Vec3::new(4.5 / 32.0, 1.0 - 4.5 / 32.0, 53.0 / 64.0), 1e-5)
        );
        assert!((position(&display[5]).x - 54.5 / 32.0).abs() < 1e-5);
        assert!((position(&display[0]).y - position(&display[6]).y - 0.5).abs() < 1e-5);
    }

    #[test]
    fn flap_display_extends_from_the_top_left_in_all_facings() {
        let mut state = FlapDisplayState::default();
        let packet = tag(
            r#"{Controller:1b,XSize:2,YSize:3,Display0:{Key:"Default",Sections:[{Width:14f,Text:'"AB"'}]}}"#,
        );
        assert!(apply_flap_display_tag(&mut state, &packet, 0));
        for (facing, axis) in [
            ("south", Vec3::X),
            ("north", Vec3::NEG_X),
            ("east", Vec3::NEG_Z),
            ("west", Vec3::Z),
        ] {
            let (min, max) = flap_display_bounds(&state, facing);
            let extent = Vec3::ONE + Vec3::Y * 2.0 + axis.abs();
            assert!((max - min).abs_diff_eq(extent, 1e-5));
            assert!((min.y + 2.0).abs() < 1e-5);
            let display = calls(&state, facing, 0.0, 0.0);
            assert!(
                (position(&display[1]) - position(&display[0]))
                    .abs_diff_eq(axis * 7.0 / 32.0, 1e-5)
            );
        }
        assert!(!apply_flap_display_tag(
            &mut state,
            &tag("{Controller:0b}"),
            1
        ));
        assert!(calls(&state, "south", 1.0, 0.0).is_empty());
    }

    #[test]
    fn flap_display_clips_single_and_wide_flaps_and_retains_unsent_text() {
        let mut state = FlapDisplayState::default();
        let packet = tag(
            r#"{Controller:1b,XSize:2,YSize:1,Display0:{Key:"Custom",Sections:[{Width:13f,SingleFlap:1b,Text:'"abcde"'},{Width:18f,Wide:1b,Text:'"fghi"'}]}}"#,
        );
        assert!(apply_flap_display_tag(&mut state, &packet, 0));
        let display = calls(&state, "south", 0.0, 0.0);
        assert_eq!(
            display
                .iter()
                .map(|call| call.text.as_str())
                .collect::<String>(),
            "abFG"
        );
        assert!((display[3].x - display[2].x - 9.0).abs() < 1e-5);
        let update = tag(
            r#"{Controller:1b,XSize:2,YSize:1,Display0:{Key:"Custom",Sections:[{Width:13f,SingleFlap:1b},{Width:18f,Wide:1b,Text:'""'}]}}"#,
        );
        assert!(apply_flap_display_tag(&mut state, &update, 4));
        assert_eq!(
            calls(&state, "south", 4.0, 0.0)
                .iter()
                .map(|call| call.text.as_str())
                .collect::<String>(),
            "ab"
        );
    }

    #[test]
    fn flap_display_cycles_changed_flaps_only_at_operating_speed() {
        let mut state = FlapDisplayState::default();
        let old = tag(
            r#"{Controller:1b,XSize:1,YSize:1,Display0:{Key:"Default",Sections:[{Width:14f,Cycle:"__test_missing",Text:'"AB"'}]}}"#,
        );
        let update = tag(
            r#"{Controller:1b,XSize:1,YSize:1,Display0:{Key:"Default",Sections:[{Width:14f,Cycle:"__test_missing",Text:'"AC"',Transition:1b}]}}"#,
        );
        assert!(apply_flap_display_tag(&mut state, &old, 0));
        assert!(apply_flap_display_tag(&mut state, &update, 8));
        let spinning = calls(&state, "south", 9.0, 32.0);
        assert_eq!(spinning[0].text, "A");
        assert_ne!(spinning[1].text, "C");
        for (time, speed) in [(9.0, 8.0), (9.0, 256.0), (30.0, 32.0)] {
            assert_eq!(calls(&state, "south", time, speed)[1].text, "C");
        }
    }
}

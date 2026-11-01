//! Legacy §-format chat <-> modern 26.3 component model.
//!
//! Two directions, one round-trip contract:
//!
//! * [`legacy_to_components`] parses a legacy `§`-encoded string (the format
//!   every server below ~1.16 used, and which plugins still emit today) into
//!   a flat run list of [`ChatNode`]s — the shape a 26.3 component tree is
//!   built from.
//! * [`flatten_components`] converts a node list back into a `§`-string for
//!   servers that expect legacy text (e.g. clientbound "we are old" paths
//!   and some plugin channels).
//!
//! The pair is *lossless on the subset we model*: parsing the flattened
//! output reproduces the same node list. That property is enforced by tests
//! and is what the wiring step will rely on when it routes chat between a
//! legacy server and the 26.3 client.
//!
//! RU: модуль двусторонней конвертации чата между legacy §-кодировкой и
//! компонентной моделью 26.3. Пара parse/flatten обратима на моделируемом
//! подмножестве — это проверяется тестами и будет опорой для wiring-этапа.
//!
//! ## Semantic notes /RU: о семантике
//!
//! * `0-9`, `a-f` are the 16 legacy color codes; they map to the modern
//!   *named* colors (`black`, `dark_blue`, ..., `white`). The 1.16+ §x-hex
//!  escape (`§x§R§R§G§G§B§B`) is NOT parsed yet — see the limitation note
//!  at the bottom of this doc.
//! * In vanilla, a color code also clears active formatting. We deliberately
//!   do NOT do that: a color code only sets the color, only `§r` resets
//!  formatting. Why: [`flatten_components`] emits the color code *last*
//!  (after the format codes), and with vanilla "color clears formats"
//!  semantics that output would re-parse differently and the round-trip
//!  would break. Pure "color sets color" is the lossless choice for this
//!  parser contract; if the wiring step needs vanilla-exact interpretation
//!  of *server* strings it can post-process the node list.
//! * Unknown codes (`§z`, `§u`, ...) are ignored: the `§` and the code
//!  character are both dropped from the output. A trailing lone `§` is
//!  dropped as well.
//! * Codes are case-insensitive (`§L` == `§l`).

/// One styled run of chat text.
///
/// A node is a piece of literal text plus the style active on it. The 26.3
/// component tree is built by turning each node into a styled text component
/// (done at wiring time, see [`components_to_263_nbt`]).
///
/// RU: один форматированный фрагмент текста: литеральный текст + активный
/// на нём стиль.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ChatNode {
    /// The literal text of this run. Never empty for nodes produced by
    /// [`legacy_to_components`].
    /// RU: литеральный текст фрагмента (у парсера — никогда не пустой).
    pub text: String,
    /// Modern color name (`"red"`, `"dark_purple"`, ...) or `None` for the
    /// client default (white).
    /// RU: имя цвета в modern-номенклатуре или None (цвет по умолчанию).
    pub color: Option<&'static str>,
    /// Bold (`§l`).
    pub bold: bool,
    /// Italic (`§o`).
    pub italic: bool,
    /// Underlined (`§n`).
    pub underline: bool,
    /// Strikethrough (`§m`).
    pub strikethrough: bool,
    /// Obfuscated / "magic" (`§k`).
    pub obfuscated: bool,
}

/// Internal style state shared by the parser and the flattener.
/// EN: kept private — the public surface is [`ChatNode`] only.
/// RU: внутреннее состояние стиля; публичная поверхность — только ChatNode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Style {
    color: Option<&'static str>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    obfuscated: bool,
}

impl Style {
    /// Apply one legacy `§` code (already lowercased) to this style.
    /// Returns `false` for unknown codes (style left untouched).
    ///
    /// EN: semantics are the lossless subset documented at the module level —
    /// a color code sets only the color, `r` clears everything.
    ///
    /// RU: применить один §-код; семантика — «цвет только красит, сбрасывает
    /// только §r» (см. доку модуля).
    fn apply_code(&mut self, code: char) -> bool {
        match code {
            'r' => {
                *self = Style::default();
                true
            }
            'l' => {
                self.bold = true;
                true
            }
            'o' => {
                self.italic = true;
                true
            }
            'n' => {
                self.underline = true;
                true
            }
            'm' => {
                self.strikethrough = true;
                true
            }
            'k' => {
                self.obfuscated = true;
                true
            }
            '0'..='9' | 'a'..='f' => {
                self.color = color_name(code);
                true
            }
            _ => false,
        }
    }

    /// Build a node for `text` carrying this style.
    /// RU: собрать узел с этим стилем для текста text.
    fn node(&self, text: String) -> ChatNode {
        ChatNode {
            text,
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            obfuscated: self.obfuscated,
        }
    }
}

/// The style of a node, as the flattener sees it.
/// RU: стиль узла глазами флэттенера.
fn node_style(node: &ChatNode) -> Style {
    Style {
        color: node.color,
        bold: node.bold,
        italic: node.italic,
        underline: node.underline,
        strikethrough: node.strikethrough,
        obfuscated: node.obfuscated,
    }
}

/// Legacy color code -> modern color name.
/// RU: legacy-код цвета -> modern-имя цвета.
fn color_name(code: char) -> Option<&'static str> {
    Some(match code {
        '0' => "black",
        '1' => "dark_blue",
        '2' => "dark_green",
        '3' => "dark_aqua",
        '4' => "dark_red",
        '5' => "dark_purple",
        '6' => "gold",
        '7' => "gray",
        '8' => "dark_gray",
        '9' => "blue",
        'a' => "green",
        'b' => "aqua",
        'c' => "red",
        'd' => "light_purple",
        'e' => "yellow",
        'f' => "white",
        _ => return None,
    })
}

/// Modern color name -> legacy color code, where one exists.
///
/// Returns `None` for modern-only colors (e.g. hex values): those are
/// dropped by [`flatten_components`] — the flattener is lossy on colors the
/// legacy palette cannot express.
///
/// RU: modern-имя цвета -> legacy-код; цвета вне legacy-палитры — None.
fn legacy_code(color: &str) -> Option<char> {
    Some(match color {
        "black" => '0',
        "dark_blue" => '1',
        "dark_green" => '2',
        "dark_aqua" => '3',
        "dark_red" => '4',
        "dark_purple" => '5',
        "gold" => '6',
        "gray" => '7',
        "dark_gray" => '8',
        "blue" => '9',
        "green" => 'a',
        "aqua" => 'b',
        "red" => 'c',
        "light_purple" => 'd',
        "yellow" => 'e',
        "white" => 'f',
        _ => return None,
    })
}

const SECTION: char = '\u{00A7}';

/// Parse a legacy `§`-encoded string into styled nodes.
///
/// Runs of text between style changes become one [`ChatNode`] each; the
/// output never contains empty-text nodes. Unknown codes and a trailing
/// lone `§` are dropped. Parsing cannot fail — the §-format has no invalid
/// input, which is why this returns a plain `Vec` and not a `Result`.
///
/// RU: разобрать legacy §-строку в узлы. Разбор не падает — поэтому без
/// Result (см. контракт модуля: ViaError нужен только там, где есть фейл).
pub fn legacy_to_components(text: &str) -> Vec<ChatNode> {
    let mut nodes: Vec<ChatNode> = Vec::new();
    let mut style = Style::default();
    let mut current = String::new();

    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != SECTION {
            current.push(ch);
            continue;
        }
        // § at end of input: drop it.
        // RU: § в конце строки — просто выбрасываем.
        let Some(raw_code) = chars.next() else {
            break;
        };
        let code = raw_code.to_ascii_lowercase();
        let old = style;
        if style.apply_code(code) && style != old && !current.is_empty() {
            // Style changed mid-run: flush the run with the OLD style.
            // RU: стиль сменился посреди текста — сбрасываем накопленное
            // со СТАРЫМ стилем.
            nodes.push(old.node(std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        nodes.push(style.node(current));
    }
    nodes
}

/// Convert nodes back into a legacy `§`-encoded string.
///
/// Per node, the codes are emitted in a fixed order — `bold`, `italic`,
/// `underline`, `strikethrough`, `obfuscated`, then the color — and a `§r`
/// is emitted between two nodes whenever their styles differ (the first
/// node gets no `§r`). Nodes with the same style are joined without codes;
/// empty-text nodes are skipped. Colors without a legacy code are dropped.
///
/// Round-trip guarantee: `legacy_to_components(flatten_components(nodes))`
/// reproduces `nodes` whenever every color is in the legacy palette and no
/// node has empty text.
///
/// RU: собрать из узлов legacy §-строку. Порядок кодов фиксирован; §r
/// вставляется между узлами при смене стиля; круглый прогон обратим.
pub fn flatten_components(nodes: &[ChatNode]) -> String {
    let mut out = String::new();
    let mut prev: Option<Style> = None;
    for node in nodes {
        if node.text.is_empty() {
            continue;
        }
        let style = node_style(node);
        if prev != Some(style) {
            if prev.is_some() {
                out.push(SECTION);
                out.push('r');
            }
            if node.bold {
                out.push(SECTION);
                out.push('l');
            }
            if node.italic {
                out.push(SECTION);
                out.push('o');
            }
            if node.underline {
                out.push(SECTION);
                out.push('n');
            }
            if node.strikethrough {
                out.push(SECTION);
                out.push('m');
            }
            if node.obfuscated {
                out.push(SECTION);
                out.push('k');
            }
            if let Some(code) = node.color.and_then(legacy_code) {
                out.push(SECTION);
                out.push(code);
            }
        }
        out.push_str(&node.text);
        prev = Some(style);
    }
    out
}

/// Encode a node list as the 26.3 NBT component tree.
///
/// FIXME(wiring): this is a placeholder — the wiring step writes the real
/// 26.3 NBT component tree here (component array with `text` / `color` /
/// `bold` / ... tags). The byte format is deliberately NOT invented in this
/// module: it must match `horizon-protocol`'s 26.3 NBT writer, which does
/// not exist on the wiring day schedule for this file. Until then this
/// returns an empty vector so callers can already route through it.
///
/// RU: заглушка под wiring — реальное NBT-дерево компонентов 26.3 напишет
/// этап wiring, чтобы не выдумывать формат тут. Пока возвращает пустой Vec.
pub fn components_to_263_nbt(nodes: &[ChatNode]) -> Vec<u8> {
    let _ = nodes;
    Vec::new()
}

/// How chat messages are carried on the wire for a protocol version.
///
/// This is a *coarse* per-version classification used by the wiring step to
/// pick the codec family; the real matrix is per-packet, not per-version
/// (see [`chat_shape`]).
///
/// RU: форма кодирования чата на проводе — грубая, по версии; реальная
/// матрица — по пакетам (см. chat_shape).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChatWireShape {
    /// Chat travels as JSON text components / signed chat components
    /// (1.19+ chat system).
    /// RU: JSON-компоненты (система чата 1.19+).
    JsonString,
    /// Chat travels as plain strings with legacy `§` formatting in at least
    /// some packets of the version (pre-1.19 servers and plugin channels).
    /// RU: обычные строки с §-кодировкой (сервера до 1.19 и каналы плагинов).
    LegacyString,
}

/// Coarse per-version chat wire shape.
///
/// Boundary: 1.19 (protocol 759) introduced the signed chat component
/// system, so 759 and above are [`ChatWireShape::JsonString`]; everything
/// below is [`ChatWireShape::LegacyString`].
///
/// FIXME(wiring): the real "which packet uses which encoding at which
/// version" matrix belongs to the wiring stage — e.g. clientbound *system*
/// chat has been JSON components long before 1.19, while plugin/plugin
/// message channels kept legacy strings after it. This module only provides
/// the primitive; the per-packet decision table lives with the packet
/// codecs.
///
/// RU: до 759 (1.19) — legacy, 759+ — JSON. Реальная матрица
/// «пакет × версия» — таблица wiring-этапа; здесь только примитив.
pub fn chat_shape(protocol: i32) -> ChatWireShape {
    // 1.19 = protocol 759 (chat rewrite with signed components).
    if protocol >= 759 {
        ChatWireShape::JsonString
    } else {
        ChatWireShape::LegacyString
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// parse -> flatten -> parse must reproduce the same node list.
    /// RU: круглый прогон обязан воспроизводить тот же список узлов.
    fn assert_round_trip(input: &str) {
        let nodes = legacy_to_components(input);
        let flat = flatten_components(&nodes);
        assert_eq!(
            legacy_to_components(&flat),
            nodes,
            "round trip failed for {:?} (flattened to {:?})",
            input,
            flat
        );
    }

    #[test]
    fn golden_full_run_dark_red() {
        // "§4Dark §lRed§r plain" -> 2 styled nodes + 1 plain node.
        // RU: полный прогон: 2 форматных узла + plain.
        let nodes = legacy_to_components("§4Dark §lRed§r plain");
        assert_eq!(
            nodes,
            vec![
                ChatNode {
                    text: "Dark ".to_string(),
                    color: Some("dark_red"),
                    ..Default::default()
                },
                ChatNode {
                    text: "Red".to_string(),
                    color: Some("dark_red"),
                    bold: true,
                    ..Default::default()
                },
                ChatNode {
                    text: " plain".to_string(),
                    ..Default::default()
                },
            ]
        );

        // Golden snapshot of the flatten output.
        // RU: золотой снапшот обратной конвертации.
        let flat = flatten_components(&nodes);
        assert_eq!(flat, "§4Dark §r§l§4Red§r plain");

        // Every code from the original run is still present.
        // RU: все коды из исходной строки на месте.
        assert!(flat.contains('§'));
        assert!(flat.contains("§4"));
        assert!(flat.contains("§l"));
        assert!(flat.contains("§r"));

        assert_round_trip("§4Dark §lRed§r plain");
    }

    #[test]
    fn golden_reset_mid_string() {
        // Reset in the middle splits the run and needs no codes on the
        // trailing plain node.
        // RU: reset посреди строки; хвостовой узел — без кодов.
        let nodes = legacy_to_components("§c§lHot§rCold");
        assert_eq!(
            nodes,
            vec![
                ChatNode {
                    text: "Hot".to_string(),
                    color: Some("red"),
                    bold: true,
                    ..Default::default()
                },
                ChatNode {
                    text: "Cold".to_string(),
                    ..Default::default()
                },
            ]
        );
        let flat = flatten_components(&nodes);
        assert_eq!(flat, "§l§cHot§rCold");
        assert_round_trip("§c§lHot§rCold");
    }

    #[test]
    fn green_code_lowercase_a() {
        // 'a' is green, not a format code.
        // RU: код 'a' — это green, а не формат.
        let nodes = legacy_to_components("§ago");
        assert_eq!(
            nodes,
            vec![ChatNode {
                text: "go".to_string(),
                color: Some("green"),
                ..Default::default()
            }]
        );
        assert_eq!(flatten_components(&nodes), "§ago");
        assert_round_trip("§ago");
    }

    #[test]
    fn unknown_code_is_ignored() {
        // §z is not a code: the pair is dropped, the text survives.
        // RU: §z не код — пара выбрасывается, текст остаётся.
        let nodes = legacy_to_components("§ztext");
        assert_eq!(
            nodes,
            vec![ChatNode {
                text: "text".to_string(),
                ..Default::default()
            }]
        );
        assert_eq!(flatten_components(&nodes), "text");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(legacy_to_components(""), Vec::<ChatNode>::new());
        assert_eq!(flatten_components(&[]), "");
    }

    #[test]
    fn consecutive_codes_produce_one_node() {
        // Stacked codes with no text between them collapse into one style.
        // RU: подряд идущие коды без текста схлопываются в один стиль.
        let nodes = legacy_to_components("§4§l§kBoom");
        assert_eq!(
            nodes,
            vec![ChatNode {
                text: "Boom".to_string(),
                color: Some("dark_red"),
                bold: true,
                obfuscated: true,
                ..Default::default()
            }]
        );
        // Obfuscated comes before the color in the fixed emit order.
        // RU: obfuscated идёт раньше цвета в фиксированном порядке.
        assert_eq!(flatten_components(&nodes), "§l§k§4Boom");
        assert_round_trip("§4§l§kBoom");
    }

    #[test]
    fn all_sixteen_colors_round_trip() {
        // Every legacy color code survives a parse/flatten round trip.
        // RU: все 16 кодов цветов проходят круглый прогон.
        for code in "0123456789abcdef".chars() {
            let input = format!("{SECTION}{code}x");
            let nodes = legacy_to_components(&input);
            assert_eq!(nodes.len(), 1, "one node for code §{code}");
            assert_eq!(nodes[0].text, "x");
            assert!(nodes[0].color.is_some(), "code §{code} must set a color");
            assert_eq!(flatten_components(&nodes), input);
            assert_round_trip(&input);
        }
    }

    #[test]
    fn all_format_codes_set_flags() {
        let nodes = legacy_to_components("§l§o§n§m§k!§r.");
        assert_eq!(
            nodes[0],
            ChatNode {
                text: "!".to_string(),
                bold: true,
                italic: true,
                underline: true,
                strikethrough: true,
                obfuscated: true,
                ..Default::default()
            }
        );
        assert_eq!(
            nodes[1],
            ChatNode {
                text: ".".to_string(),
                ..Default::default()
            }
        );
        assert_round_trip("§l§o§n§m§k!§r.");
    }

    #[test]
    fn uppercase_codes_are_accepted() {
        // §L == §l, §M == §m (both case-insensitive).
        // RU: §L эквивалентно §l, §M — §m; коды нечувствительны к регистру.
        let nodes = legacy_to_components("§LBO§MLD");
        assert_eq!(
            nodes,
            vec![
                ChatNode {
                    text: "BO".to_string(),
                    bold: true,
                    ..Default::default()
                },
                ChatNode {
                    text: "LD".to_string(),
                    bold: true,
                    strikethrough: true,
                    ..Default::default()
                },
            ]
        );
        assert_eq!(flatten_components(&nodes), "§lBO§r§l§mLD");
        assert_round_trip("§LBO§MLD");
    }

    #[test]
    fn trailing_section_sign_is_dropped() {
        let nodes = legacy_to_components("abc§");
        assert_eq!(
            nodes,
            vec![ChatNode {
                text: "abc".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn same_style_nodes_join_without_codes() {
        // Two nodes with identical style emit the codes once.
        // RU: узлы с одинаковым стилем — коды один раз, без §r.
        let nodes = vec![
            ChatNode {
                text: "a".to_string(),
                color: Some("gold"),
                bold: true,
                ..Default::default()
            },
            ChatNode {
                text: "b".to_string(),
                color: Some("gold"),
                bold: true,
                ..Default::default()
            },
        ];
        assert_eq!(flatten_components(&nodes), "§l§6ab");
    }

    #[test]
    fn color_without_legacy_code_is_dropped() {
        // A modern-only color cannot be expressed in legacy format: the
        // text survives, the color does not (documented lossiness).
        // RU: цвет вне legacy-палитры теряется, текст остаётся.
        let nodes = vec![ChatNode {
            text: "hexed".to_string(),
            color: Some("amber_red"),
            ..Default::default()
        }];
        assert_eq!(flatten_components(&nodes), "hexed");
    }

    #[test]
    fn empty_text_nodes_are_skipped_by_flatten() {
        let nodes = vec![
            ChatNode {
                text: String::new(),
                bold: true,
                ..Default::default()
            },
            ChatNode {
                text: "real".to_string(),
                ..Default::default()
            },
        ];
        assert_eq!(flatten_components(&nodes), "real");
    }

    #[test]
    fn repeated_code_is_idempotent() {
        // §l§l is one bold, no zero-length node in between.
        // RU: повтор кода не создаёт пустых узлов.
        let nodes = legacy_to_components("§l§lx");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].bold);
        assert_eq!(flatten_components(&nodes), "§lx");
    }

    #[test]
    fn nbt_stub_returns_empty() {
        // The 26.3 NBT writer is a wiring-day FIXME; until then: empty.
        // RU: заглушка NBT — пустой Vec до wiring-этапа.
        let nodes = legacy_to_components("§4Dark §lRed§r plain");
        assert!(components_to_263_nbt(&nodes).is_empty());
        assert!(components_to_263_nbt(&[]).is_empty());
    }

    #[test]
    fn chat_shape_by_protocol() {
        // 1.8.8 (47) and 1.16.5 (754) are legacy; 1.19 (759) and anything
        // newer are JSON; the boundary itself lands on JsonString.
        // RU: 47 и 754 — legacy; 759 (1.19) и выше — JSON.
        assert_eq!(chat_shape(47), ChatWireShape::LegacyString);
        assert_eq!(chat_shape(754), ChatWireShape::LegacyString);
        assert_eq!(chat_shape(758), ChatWireShape::LegacyString);
        assert_eq!(chat_shape(759), ChatWireShape::JsonString);
        assert_eq!(chat_shape(999_999), ChatWireShape::JsonString);
    }

    #[test]
    fn plain_text_without_codes_is_one_node() {
        let nodes = legacy_to_components("just text");
        assert_eq!(
            nodes,
            vec![ChatNode {
                text: "just text".to_string(),
                ..Default::default()
            }]
        );
        assert_eq!(flatten_components(&nodes), "just text");
    }
}

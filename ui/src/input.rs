// SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception
//! The input-method server's logic (milestone 7b, step 3; decision 50):
//! between the keyboard and the apps. Apps never read the keyboard for
//! text; the desktop turns key presses into text here and hands the
//! window in front only what was typed.
//!
//! - **Layouts**: US, UK, German, French, Italian and Spanish, with AltGr.
//! - **Dead keys**: ´ then e gives é; a dead key then space gives the mark.
//! - **Compose** (the Compose or Menu key, then two keys): æ, ß, €, «, ½...
//! - **Japanese**: romaji to hiragana as you type (nihongo: にほんご),
//!   Enter or Space commits.
//! - **Chinese**: pinyin with a list of candidates; Space takes the first,
//!   a digit takes that one, Enter keeps the letters.
//!
//! Pure logic, tested on the host.

use alloc::string::String;
use alloc::vec::Vec;

/// Key codes (Linux input numbers, as virtio sends them) used here.
pub mod code {
    pub const ESC: u16 = 1;
    pub const BACKSPACE: u16 = 14;
    pub const TAB: u16 = 15;
    pub const ENTER: u16 = 28;
    pub const SPACE: u16 = 57;
    pub const ALTGR: u16 = 100;
    pub const COMPOSE: u16 = 127;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Us,
    Uk,
    German,
    French,
    Italian,
    Spanish,
}

impl Layout {
    pub const ALL: [Layout; 6] = [
        Layout::Us,
        Layout::Uk,
        Layout::German,
        Layout::French,
        Layout::Italian,
        Layout::Spanish,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> Layout {
        *Self::ALL.get(c as usize).unwrap_or(&Layout::Us)
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Us => "English (US)",
            Layout::Uk => "English (UK)",
            Layout::German => "Deutsch",
            Layout::French => "Français",
            Layout::Italian => "Italiano",
            Layout::Spanish => "Español",
        }
    }
}

/// What a key gives: a character, or a dead key waiting for the next one
/// (its combining mark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sym {
    Char(char),
    Dead(char),
}

const ACUTE: char = '\u{301}';
const GRAVE: char = '\u{300}';
const CIRCUMFLEX: char = '\u{302}';
const DIAERESIS: char = '\u{308}';
const TILDE: char = '\u{303}';

/// The US keyboard, by key code (2 to 53): without and with Shift.
const US_LOWER: &[u8; 52] = b"1234567890-=\0\0qwertyuiop[]\0\0asdfghjkl;'`\0\\zxcvbnm,./";
const US_UPPER: &[u8; 52] = b"!@#$%^&*()_+\0\0QWERTYUIOP{}\0\0ASDFGHJKL:\"~\0|ZXCVBNM<>?";

fn us(code: u16, shift: bool) -> Option<char> {
    if code == code::SPACE {
        return Some(' ');
    }
    let i = (code as usize).checked_sub(2)?;
    let b = *if shift { US_UPPER } else { US_LOWER }.get(i)?;
    (b != 0).then_some(b as char)
}

/// What key `code` gives on `layout`, with Shift and AltGr as held.
pub fn sym(layout: Layout, code: u16, shift: bool, altgr: bool) -> Option<Sym> {
    use Sym::{Char as C, Dead as D};
    let c = |ch: char| Some(C(ch));
    // The extra key next to left Shift on European keyboards.
    const ISO: u16 = 86;
    match layout {
        Layout::Us => {}
        Layout::Uk => match (code, shift) {
            (3, true) => return c('"'),
            (4, true) => return c('£'),
            (40, false) => return c('\''),
            (40, true) => return c('@'),
            (43, false) => return c('#'),
            (43, true) => return c('~'),
            (41, true) => return c('¬'),
            (ISO, false) => return c('\\'),
            (ISO, true) => return c('|'),
            _ => {}
        },
        Layout::German => {
            if altgr {
                return match code {
                    16 => c('@'),
                    18 => c('€'),
                    8 => c('{'),
                    9 => c('['),
                    10 => c(']'),
                    11 => c('}'),
                    12 => c('\\'),
                    27 => c('~'),
                    ISO => c('|'),
                    _ => None,
                };
            }
            const SHIFTED: [char; 10] = ['!', '"', '§', '$', '%', '&', '/', '(', ')', '='];
            return match (code, shift) {
                (3..=11, true) => c(SHIFTED[code as usize - 2]),
                (2, true) => c('!'),
                (12, false) => c('ß'),
                (12, true) => c('?'),
                (13, false) => Some(D(ACUTE)),
                (13, true) => Some(D(GRAVE)),
                (21, s) => c(if s { 'Z' } else { 'z' }),
                (44, s) => c(if s { 'Y' } else { 'y' }),
                (26, s) => c(if s { 'Ü' } else { 'ü' }),
                (27, s) => c(if s { '*' } else { '+' }),
                (39, s) => c(if s { 'Ö' } else { 'ö' }),
                (40, s) => c(if s { 'Ä' } else { 'ä' }),
                (41, false) => Some(D(CIRCUMFLEX)),
                (41, true) => c('°'),
                (43, s) => c(if s { '\'' } else { '#' }),
                (51, s) => c(if s { ';' } else { ',' }),
                (52, s) => c(if s { ':' } else { '.' }),
                (53, s) => c(if s { '_' } else { '-' }),
                (ISO, s) => c(if s { '>' } else { '<' }),
                _ => us(code, shift).map(C),
            };
        }
        Layout::French => {
            if altgr {
                return match code {
                    3 => Some(D(TILDE)),
                    4 => c('#'),
                    5 => c('{'),
                    6 => c('['),
                    7 => c('|'),
                    8 => Some(D(GRAVE)),
                    9 => c('\\'),
                    10 => c('^'),
                    11 => c('@'),
                    12 => c(']'),
                    13 => c('}'),
                    18 => c('€'),
                    _ => None,
                };
            }
            const LOWER: [char; 12] = ['&', 'é', '"', '\'', '(', '-', 'è', '_', 'ç', 'à', ')', '='];
            const UPPER: [char; 12] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '°', '+'];
            let letter = |l: char| c(if shift { l.to_ascii_uppercase() } else { l });
            return match code {
                2..=13 => c(if shift { UPPER } else { LOWER }[code as usize - 2]),
                16 => letter('a'),
                17 => letter('z'),
                30 => letter('q'),
                39 => letter('m'),
                44 => letter('w'),
                26 => Some(D(if shift { DIAERESIS } else { CIRCUMFLEX })),
                27 => c(if shift { '£' } else { '$' }),
                40 => c(if shift { '%' } else { 'ù' }),
                43 => c(if shift { 'µ' } else { '*' }),
                41 => c('²'),
                50 => c(if shift { '?' } else { ',' }),
                51 => c(if shift { '.' } else { ';' }),
                52 => c(if shift { '/' } else { ':' }),
                53 => c(if shift { '§' } else { '!' }),
                ISO => c(if shift { '>' } else { '<' }),
                _ => us(code, shift).map(C),
            };
        }
        Layout::Italian => {
            if altgr {
                return match code {
                    26 => c('['),
                    27 => c(']'),
                    39 => c('@'),
                    40 => c('#'),
                    18 => c('€'),
                    _ => None,
                };
            }
            const SHIFTED: [char; 10] = ['!', '"', '£', '$', '%', '&', '/', '(', ')', '='];
            return match (code, shift) {
                (2..=11, true) => c(SHIFTED[code as usize - 2]),
                (12, s) => c(if s { '?' } else { '\'' }),
                (13, s) => c(if s { '^' } else { 'ì' }),
                (26, s) => c(if s { 'é' } else { 'è' }),
                (27, s) => c(if s { '*' } else { '+' }),
                (39, s) => c(if s { 'ç' } else { 'ò' }),
                (40, s) => c(if s { '°' } else { 'à' }),
                (43, s) => c(if s { '§' } else { 'ù' }),
                (41, s) => c(if s { '|' } else { '\\' }),
                (51, s) => c(if s { ';' } else { ',' }),
                (52, s) => c(if s { ':' } else { '.' }),
                (53, s) => c(if s { '_' } else { '-' }),
                (ISO, s) => c(if s { '>' } else { '<' }),
                _ => us(code, shift).map(C),
            };
        }
        Layout::Spanish => {
            if altgr {
                return match code {
                    2 => c('|'),
                    3 => c('@'),
                    4 => c('#'),
                    26 => c('['),
                    27 => c(']'),
                    18 => c('€'),
                    _ => None,
                };
            }
            const SHIFTED: [char; 10] = ['!', '"', '·', '$', '%', '&', '/', '(', ')', '='];
            return match (code, shift) {
                (2..=11, true) => c(SHIFTED[code as usize - 2]),
                (12, s) => c(if s { '?' } else { '\'' }),
                (13, s) => c(if s { '¿' } else { '¡' }),
                (26, s) => Some(D(if s { CIRCUMFLEX } else { GRAVE })),
                (27, s) => c(if s { '*' } else { '+' }),
                (39, s) => c(if s { 'Ñ' } else { 'ñ' }),
                (40, s) => Some(D(if s { DIAERESIS } else { ACUTE })),
                (43, s) => c(if s { 'Ç' } else { 'ç' }),
                (41, s) => c(if s { 'ª' } else { 'º' }),
                (51, s) => c(if s { ';' } else { ',' }),
                (52, s) => c(if s { ':' } else { '.' }),
                (53, s) => c(if s { '_' } else { '-' }),
                (ISO, s) => c(if s { '>' } else { '<' }),
                _ => us(code, shift).map(C),
            };
        }
    }
    if altgr {
        return None;
    }
    us(code, shift).map(C)
}

/// A mark on a letter: the letter that has it, if Latin has one.
pub fn combine(mark: char, base: char) -> Option<char> {
    const TABLE: &[(char, &str, &str)] = &[
        (ACUTE, "aeiouyAEIOUYcnCN", "áéíóúýÁÉÍÓÚÝćńĆŃ"),
        (GRAVE, "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        (CIRCUMFLEX, "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        (DIAERESIS, "aeiouyAEIOUY", "äëïöüÿÄËÏÖÜŸ"),
        (TILDE, "anoANO", "ãñõÃÑÕ"),
    ];
    let (_, from, to) = TABLE.iter().find(|t| t.0 == mark)?;
    let i = from.chars().position(|c| c == base)?;
    to.chars().nth(i)
}

/// The mark on its own (a dead key, then Space).
pub fn spacing(mark: char) -> char {
    match mark {
        ACUTE => '´',
        GRAVE => '`',
        CIRCUMFLEX => '^',
        DIAERESIS => '¨',
        TILDE => '~',
        c => c,
    }
}

/// Compose, then two keys: the character they make.
pub fn compose(a: char, b: char) -> Option<char> {
    const PAIRS: &[(&str, char)] = &[
        ("ae", 'æ'),
        ("AE", 'Æ'),
        ("oe", 'œ'),
        ("OE", 'Œ'),
        ("ss", 'ß'),
        ("o/", 'ø'),
        ("O/", 'Ø'),
        ("aa", 'å'),
        ("AA", 'Å'),
        ("e=", '€'),
        ("=e", '€'),
        ("l-", '£'),
        ("y=", '¥'),
        ("c,", 'ç'),
        ("C,", 'Ç'),
        ("<<", '«'),
        (">>", '»'),
        ("12", '½'),
        ("14", '¼'),
        ("34", '¾'),
        ("tm", '™'),
        ("co", '©'),
        ("ro", '®'),
        ("??", '¿'),
        ("!!", '¡'),
        ("oo", '°'),
        ("+-", '±'),
        ("xx", '×'),
        (":-", '÷'),
        ("..", '…'),
        ("-.", '·'),
        ("--", '–'),
    ];
    let mut key = String::new();
    key.push(a);
    key.push(b);
    if let Some(&(_, c)) = PAIRS.iter().find(|p| p.0 == key) {
        return Some(c);
    }
    // A mark sign, then a letter: ' ` ^ " ~ as dead keys.
    let mark = match a {
        '\'' => ACUTE,
        '`' => GRAVE,
        '^' => CIRCUMFLEX,
        '"' => DIAERESIS,
        '~' => TILDE,
        _ => return None,
    };
    combine(mark, b)
}

/// Romaji to hiragana: (romaji, kana), longest first where it matters.
const KANA: &[(&str, &str)] = &[
    ("kya", "きゃ"),
    ("kyu", "きゅ"),
    ("kyo", "きょ"),
    ("sha", "しゃ"),
    ("shu", "しゅ"),
    ("sho", "しょ"),
    ("cha", "ちゃ"),
    ("chu", "ちゅ"),
    ("cho", "ちょ"),
    ("nya", "にゃ"),
    ("nyu", "にゅ"),
    ("nyo", "にょ"),
    ("hya", "ひゃ"),
    ("hyu", "ひゅ"),
    ("hyo", "ひょ"),
    ("mya", "みゃ"),
    ("myu", "みゅ"),
    ("myo", "みょ"),
    ("rya", "りゃ"),
    ("ryu", "りゅ"),
    ("ryo", "りょ"),
    ("gya", "ぎゃ"),
    ("gyu", "ぎゅ"),
    ("gyo", "ぎょ"),
    ("bya", "びゃ"),
    ("byu", "びゅ"),
    ("byo", "びょ"),
    ("pya", "ぴゃ"),
    ("pyu", "ぴゅ"),
    ("pyo", "ぴょ"),
    ("ja", "じゃ"),
    ("ju", "じゅ"),
    ("jo", "じょ"),
    ("shi", "し"),
    ("chi", "ち"),
    ("tsu", "つ"),
    ("ka", "か"),
    ("ki", "き"),
    ("ku", "く"),
    ("ke", "け"),
    ("ko", "こ"),
    ("sa", "さ"),
    ("su", "す"),
    ("se", "せ"),
    ("so", "そ"),
    ("ta", "た"),
    ("te", "て"),
    ("to", "と"),
    ("na", "な"),
    ("ni", "に"),
    ("nu", "ぬ"),
    ("ne", "ね"),
    ("no", "の"),
    ("ha", "は"),
    ("hi", "ひ"),
    ("fu", "ふ"),
    ("he", "へ"),
    ("ho", "ほ"),
    ("ma", "ま"),
    ("mi", "み"),
    ("mu", "む"),
    ("me", "め"),
    ("mo", "も"),
    ("ya", "や"),
    ("yu", "ゆ"),
    ("yo", "よ"),
    ("ra", "ら"),
    ("ri", "り"),
    ("ru", "る"),
    ("re", "れ"),
    ("ro", "ろ"),
    ("wa", "わ"),
    ("wo", "を"),
    ("ga", "が"),
    ("gi", "ぎ"),
    ("gu", "ぐ"),
    ("ge", "げ"),
    ("go", "ご"),
    ("za", "ざ"),
    ("ji", "じ"),
    ("zu", "ず"),
    ("ze", "ぜ"),
    ("zo", "ぞ"),
    ("da", "だ"),
    ("de", "で"),
    ("do", "ど"),
    ("ba", "ば"),
    ("bi", "び"),
    ("bu", "ぶ"),
    ("be", "べ"),
    ("bo", "ぼ"),
    ("pa", "ぱ"),
    ("pi", "ぴ"),
    ("pu", "ぷ"),
    ("pe", "ぺ"),
    ("po", "ぽ"),
    ("nn", "ん"),
    ("a", "あ"),
    ("i", "い"),
    ("u", "う"),
    ("e", "え"),
    ("o", "お"),
    ("-", "ー"),
];

/// Romaji typed so far as kana, and the letters still waiting.
pub fn kana(romaji: &str) -> (String, String) {
    let mut out = String::new();
    let mut rest = romaji;
    'outer: while !rest.is_empty() {
        let b = rest.as_bytes();
        // A doubled consonant: a small tsu.
        if b.len() >= 2 && b[0] == b[1] && b[0].is_ascii_alphabetic() && !b"aeioun".contains(&b[0])
        {
            out.push('っ');
            rest = &rest[1..];
            continue;
        }
        // n before a consonant other than y: ん.
        if b.len() >= 2 && b[0] == b'n' && !b"aeiouyn".contains(&b[1]) {
            out.push('ん');
            rest = &rest[1..];
            continue;
        }
        for &(r, k) in KANA {
            if rest.starts_with(r) {
                out.push_str(k);
                rest = &rest[r.len()..];
                continue 'outer;
            }
        }
        // Could still become something: wait for more letters.
        if rest.len() <= 3 && KANA.iter().any(|(r, _)| r.starts_with(rest)) {
            break;
        }
        // Nothing can: keep the letter as it is.
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    (out, String::from(rest))
}

/// Pinyin: words first, then single syllables; each with its characters,
/// the most used first.
const PINYIN: &[(&str, &str)] = &[
    ("nihao", "你好"),
    ("xiexie", "谢谢"),
    ("zhongguo", "中国"),
    ("zhongwen", "中文"),
    ("pengyou", "朋友"),
    ("women", "我们"),
    ("nimen", "你们"),
    ("tamen", "他们"),
    ("shijie", "世界"),
    ("diannao", "电脑"),
    ("xuexi", "学习"),
    ("gongzuo", "工作"),
    ("shenme", "什么"),
    ("meiyou", "没有"),
    ("keyi", "可以"),
    ("xihuan", "喜欢"),
    ("jintian", "今天"),
    ("mingtian", "明天"),
    ("zaijian", "再见"),
    ("duibuqi", "对不起"),
    ("ni", "你尼泥"),
    ("hao", "好号毫"),
    ("wo", "我握"),
    ("shi", "是时事十市"),
    ("de", "的得德"),
    ("bu", "不部步"),
    ("zhong", "中种重"),
    ("guo", "国过果"),
    ("ren", "人认任"),
    ("da", "大打达"),
    ("xie", "谢写些"),
    ("xue", "学雪"),
    ("sheng", "生声省"),
    ("men", "们门"),
    ("zai", "在再"),
    ("you", "有又由"),
    ("le", "了乐"),
    ("he", "和合河"),
    ("ta", "他她它"),
    ("zhe", "这者"),
    ("shang", "上商"),
    ("xia", "下夏"),
    ("lai", "来"),
    ("qu", "去区"),
    ("hui", "会回"),
    ("shuo", "说"),
    ("kan", "看"),
    ("ai", "爱"),
    ("jia", "家加"),
    ("tian", "天田"),
    ("nian", "年"),
    ("yue", "月越"),
    ("ri", "日"),
    ("ming", "明名"),
    ("xin", "心新信"),
    ("kai", "开"),
    ("shui", "水谁"),
    ("huo", "火或活"),
    ("shan", "山"),
    ("yi", "一以意"),
    ("er", "二而"),
    ("san", "三"),
    ("si", "四思"),
    ("wu", "五无"),
    ("liu", "六"),
    ("qi", "七起"),
    ("ba", "八把"),
    ("jiu", "九就"),
    ("ge", "个"),
    ("mei", "没美"),
    ("na", "那哪拿"),
    ("dou", "都"),
    ("ye", "也"),
    ("dao", "到道"),
    ("yao", "要"),
    ("neng", "能"),
    ("hen", "很"),
    ("duo", "多"),
    ("xiao", "小笑"),
];

/// The first entries of [`PINYIN`] are words, the rest syllables.
const WORDS: usize = 20;

/// The characters pinyin `p` can be (words first).
pub fn pinyin(p: &str) -> Vec<String> {
    let mut v = Vec::new();
    for (i, &(k, chars)) in PINYIN.iter().enumerate() {
        if k != p {
            continue;
        }
        if i < WORDS {
            v.push(String::from(chars));
        } else {
            v.extend(chars.chars().map(String::from));
        }
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Latin,
    Japanese,
    Chinese,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Latin => "Latin letters",
            Mode::Japanese => "日本語 (kana)",
            Mode::Chinese => "中文 (pinyin)",
        }
    }

    pub fn code(self) -> u64 {
        self as u64
    }
}

/// What the input method does with a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Text for the window in front.
    Commit(String),
    /// The key itself, for the window (Enter, arrows, Backspace...).
    Pass,
    /// Taken: the input method shows what is being made.
    Held,
}

/// The input method's state: the layout, the mode, and what is being
/// made (a dead key, a compose, romaji or pinyin).
#[derive(Clone, Debug)]
pub struct Ime {
    pub layout: Layout,
    pub mode: Mode,
    dead: Option<char>,
    /// Compose was pressed: the keys after it.
    composing: Option<Vec<char>>,
    /// Letters typed for Japanese or Chinese, not yet committed.
    pub letters: String,
    /// Chinese candidates for `letters`, after Space.
    pub candidates: Vec<String>,
}

impl Ime {
    pub fn new(layout: Layout) -> Ime {
        Ime {
            layout,
            mode: Mode::Latin,
            dead: None,
            composing: None,
            letters: String::new(),
            candidates: Vec::new(),
        }
    }

    /// The next mode (Alaya key + Space). What was being made is kept as
    /// it was typed.
    pub fn next_mode(&mut self) -> Option<String> {
        let left = self.flush();
        self.mode = match self.mode {
            Mode::Latin => Mode::Japanese,
            Mode::Japanese => Mode::Chinese,
            Mode::Chinese => Mode::Latin,
        };
        left
    }

    /// What is being made, to show near the text (empty: nothing).
    pub fn preedit(&self) -> String {
        match self.mode {
            Mode::Japanese => {
                let (k, rest) = kana(&self.letters);
                k + &rest
            }
            _ => {
                let mut s = self.letters.clone();
                if let Some(d) = self.dead {
                    s.push(spacing(d));
                }
                if let Some(c) = &self.composing {
                    s.push('·');
                    s.extend(c.iter());
                }
                s
            }
        }
    }

    /// Give up what is being made, as typed.
    fn flush(&mut self) -> Option<String> {
        self.dead = None;
        self.composing = None;
        self.candidates.clear();
        let l = core::mem::take(&mut self.letters);
        (!l.is_empty()).then_some(match self.mode {
            Mode::Japanese => {
                let (k, rest) = kana(&l);
                k + &rest
            }
            _ => l,
        })
    }

    fn text(c: char) -> Out {
        let mut s = String::new();
        s.push(c);
        Out::Commit(s)
    }

    /// A key press (`code`), with Shift and AltGr as held.
    pub fn key(&mut self, code: u16, shift: bool, altgr: bool) -> Out {
        if code == code::COMPOSE {
            self.composing = Some(Vec::new());
            return Out::Held;
        }
        // Japanese and Chinese: letters gather until committed.
        if self.mode != Mode::Latin && self.composing.is_none() {
            match code {
                code::ENTER if !self.letters.is_empty() => {
                    return Out::Commit(self.flush().unwrap_or_default());
                }
                code::BACKSPACE if !self.letters.is_empty() => {
                    self.letters.pop();
                    self.candidates.clear();
                    return Out::Held;
                }
                code::ESC if !self.letters.is_empty() => {
                    self.flush();
                    return Out::Held;
                }
                code::SPACE if !self.letters.is_empty() => {
                    if self.mode == Mode::Japanese {
                        return Out::Commit(self.flush().unwrap_or_default());
                    }
                    if self.candidates.is_empty() {
                        self.candidates = pinyin(&self.letters);
                        if self.candidates.is_empty() {
                            return Out::Commit(self.flush().unwrap_or_default());
                        }
                        // Space again takes the first.
                        return Out::Held;
                    }
                    let first = self.candidates[0].clone();
                    self.flush();
                    return Out::Commit(first);
                }
                _ => {}
            }
            if let Some(Sym::Char(c)) = sym(self.layout, code, shift, altgr) {
                if c.is_ascii_lowercase() || (self.mode == Mode::Japanese && c == '-') {
                    if !self.candidates.is_empty() {
                        // A new syllable: the shown first candidate stays.
                        let first = self.candidates[0].clone();
                        self.flush();
                        self.letters.push(c);
                        return Out::Commit(first);
                    }
                    self.letters.push(c);
                    return Out::Held;
                }
                if let Some(d) = c.to_digit(10)
                    && d >= 1
                    && let Some(pick) = self.candidates.get(d as usize - 1).cloned()
                {
                    self.flush();
                    return Out::Commit(pick);
                }
                if !self.letters.is_empty() {
                    // Anything else commits what was made, then itself.
                    let mut s = self.flush().unwrap_or_default();
                    s.push(c);
                    return Out::Commit(s);
                }
            }
        }
        let Some(sym) = sym(self.layout, code, shift, altgr) else {
            self.dead = None;
            return Out::Pass;
        };
        if let Some(keys) = &mut self.composing {
            let c = match sym {
                Sym::Char(c) => c,
                Sym::Dead(m) => spacing(m),
            };
            keys.push(c);
            if keys.len() < 2 {
                return Out::Held;
            }
            let (a, b) = (keys[0], keys[1]);
            self.composing = None;
            return match compose(a, b) {
                Some(c) => Self::text(c),
                None => Out::Held,
            };
        }
        match (self.dead.take(), sym) {
            (None, Sym::Dead(m)) => {
                self.dead = Some(m);
                Out::Held
            }
            (None, Sym::Char(c)) => Self::text(c),
            (Some(m), Sym::Char(' ')) => Self::text(spacing(m)),
            (Some(m), Sym::Char(c)) => match combine(m, c) {
                Some(x) => Self::text(x),
                None => {
                    let mut s = String::new();
                    s.push(spacing(m));
                    s.push(c);
                    Out::Commit(s)
                }
            },
            (Some(m), Sym::Dead(n)) if m == n => Self::text(spacing(m)),
            (Some(_), Sym::Dead(n)) => {
                self.dead = Some(n);
                Out::Held
            }
        }
    }
}

/// The emoji picker's emoji, with their names for the screen reader.
pub const EMOJI: &[(char, &str)] = &[
    ('😀', "grinning face"),
    ('😂', "tears of joy"),
    ('😊', "smiling face"),
    ('😍', "heart eyes"),
    ('😎', "sunglasses"),
    ('😉', "winking face"),
    ('😢', "crying face"),
    ('😮', "surprised face"),
    ('😴', "sleeping face"),
    ('🙂', "slight smile"),
    ('🙃', "upside down"),
    ('🙏', "thank you"),
    ('👍', "thumbs up"),
    ('👏', "clapping"),
    ('❤', "heart"),
    ('🎉', "party"),
    ('🚀', "rocket"),
    ('💡', "idea"),
    ('🔥', "fire"),
    ('✅', "done"),
    ('🌟', "star"),
    ('🎁', "gift"),
    ('😅', "sweat smile"),
    ('😇', "halo"),
];

/// The US key code of a letter or digit (for tests and typing helpers).
pub fn tests_code(c: char) -> (u16, bool) {
    for code in 2..=53u16 {
        if us(code, false) == Some(c) {
            return (code, false);
        }
        if us(code, true) == Some(c) {
            return (code, true);
        }
    }
    (code::SPACE, false)
}

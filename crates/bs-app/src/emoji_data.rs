//! Эмодзи для выбора в композере: цветные картинки Twemoji.
//!
//! Графика Twemoji © Twitter, Inc. и участники проекта, CC-BY 4.0
//! (https://creativecommons.org/licenses/by/4.0/); файлы взяты из крейта
//! `twemoji-assets`.

/// Один эмодзи: текст для вставки и картинка 72×72.
pub(crate) struct Entry {
    pub(crate) glyph: &'static str,
    pub(crate) png: &'static [u8],
}

/// Категории: название, значок вкладки и содержимое.
pub(crate) const CATEGORIES: &[(&str, &str, &[Entry])] = &[
    (
        "Смайлы",
        "😀",
        &[
            Entry {
                glyph: "😀",
                png: include_bytes!("../assets/emoji/1f600.png"),
            },
            Entry {
                glyph: "😁",
                png: include_bytes!("../assets/emoji/1f601.png"),
            },
            Entry {
                glyph: "😂",
                png: include_bytes!("../assets/emoji/1f602.png"),
            },
            Entry {
                glyph: "😃",
                png: include_bytes!("../assets/emoji/1f603.png"),
            },
            Entry {
                glyph: "😄",
                png: include_bytes!("../assets/emoji/1f604.png"),
            },
            Entry {
                glyph: "😅",
                png: include_bytes!("../assets/emoji/1f605.png"),
            },
            Entry {
                glyph: "😆",
                png: include_bytes!("../assets/emoji/1f606.png"),
            },
            Entry {
                glyph: "😇",
                png: include_bytes!("../assets/emoji/1f607.png"),
            },
            Entry {
                glyph: "😈",
                png: include_bytes!("../assets/emoji/1f608.png"),
            },
            Entry {
                glyph: "😉",
                png: include_bytes!("../assets/emoji/1f609.png"),
            },
            Entry {
                glyph: "😊",
                png: include_bytes!("../assets/emoji/1f60a.png"),
            },
            Entry {
                glyph: "😋",
                png: include_bytes!("../assets/emoji/1f60b.png"),
            },
            Entry {
                glyph: "😌",
                png: include_bytes!("../assets/emoji/1f60c.png"),
            },
            Entry {
                glyph: "😍",
                png: include_bytes!("../assets/emoji/1f60d.png"),
            },
            Entry {
                glyph: "😎",
                png: include_bytes!("../assets/emoji/1f60e.png"),
            },
            Entry {
                glyph: "😏",
                png: include_bytes!("../assets/emoji/1f60f.png"),
            },
            Entry {
                glyph: "😐",
                png: include_bytes!("../assets/emoji/1f610.png"),
            },
            Entry {
                glyph: "😑",
                png: include_bytes!("../assets/emoji/1f611.png"),
            },
            Entry {
                glyph: "😒",
                png: include_bytes!("../assets/emoji/1f612.png"),
            },
            Entry {
                glyph: "😓",
                png: include_bytes!("../assets/emoji/1f613.png"),
            },
            Entry {
                glyph: "😔",
                png: include_bytes!("../assets/emoji/1f614.png"),
            },
            Entry {
                glyph: "😕",
                png: include_bytes!("../assets/emoji/1f615.png"),
            },
            Entry {
                glyph: "😖",
                png: include_bytes!("../assets/emoji/1f616.png"),
            },
            Entry {
                glyph: "😗",
                png: include_bytes!("../assets/emoji/1f617.png"),
            },
            Entry {
                glyph: "😘",
                png: include_bytes!("../assets/emoji/1f618.png"),
            },
            Entry {
                glyph: "😙",
                png: include_bytes!("../assets/emoji/1f619.png"),
            },
            Entry {
                glyph: "😚",
                png: include_bytes!("../assets/emoji/1f61a.png"),
            },
            Entry {
                glyph: "😛",
                png: include_bytes!("../assets/emoji/1f61b.png"),
            },
            Entry {
                glyph: "😜",
                png: include_bytes!("../assets/emoji/1f61c.png"),
            },
            Entry {
                glyph: "😝",
                png: include_bytes!("../assets/emoji/1f61d.png"),
            },
            Entry {
                glyph: "😞",
                png: include_bytes!("../assets/emoji/1f61e.png"),
            },
            Entry {
                glyph: "😟",
                png: include_bytes!("../assets/emoji/1f61f.png"),
            },
            Entry {
                glyph: "😠",
                png: include_bytes!("../assets/emoji/1f620.png"),
            },
            Entry {
                glyph: "😡",
                png: include_bytes!("../assets/emoji/1f621.png"),
            },
            Entry {
                glyph: "😢",
                png: include_bytes!("../assets/emoji/1f622.png"),
            },
            Entry {
                glyph: "😣",
                png: include_bytes!("../assets/emoji/1f623.png"),
            },
            Entry {
                glyph: "😤",
                png: include_bytes!("../assets/emoji/1f624.png"),
            },
            Entry {
                glyph: "😥",
                png: include_bytes!("../assets/emoji/1f625.png"),
            },
            Entry {
                glyph: "😦",
                png: include_bytes!("../assets/emoji/1f626.png"),
            },
            Entry {
                glyph: "😧",
                png: include_bytes!("../assets/emoji/1f627.png"),
            },
            Entry {
                glyph: "😨",
                png: include_bytes!("../assets/emoji/1f628.png"),
            },
            Entry {
                glyph: "😩",
                png: include_bytes!("../assets/emoji/1f629.png"),
            },
            Entry {
                glyph: "😪",
                png: include_bytes!("../assets/emoji/1f62a.png"),
            },
            Entry {
                glyph: "😫",
                png: include_bytes!("../assets/emoji/1f62b.png"),
            },
            Entry {
                glyph: "😬",
                png: include_bytes!("../assets/emoji/1f62c.png"),
            },
            Entry {
                glyph: "😭",
                png: include_bytes!("../assets/emoji/1f62d.png"),
            },
            Entry {
                glyph: "😮",
                png: include_bytes!("../assets/emoji/1f62e.png"),
            },
            Entry {
                glyph: "😯",
                png: include_bytes!("../assets/emoji/1f62f.png"),
            },
            Entry {
                glyph: "😰",
                png: include_bytes!("../assets/emoji/1f630.png"),
            },
            Entry {
                glyph: "😱",
                png: include_bytes!("../assets/emoji/1f631.png"),
            },
            Entry {
                glyph: "😲",
                png: include_bytes!("../assets/emoji/1f632.png"),
            },
            Entry {
                glyph: "😳",
                png: include_bytes!("../assets/emoji/1f633.png"),
            },
            Entry {
                glyph: "😴",
                png: include_bytes!("../assets/emoji/1f634.png"),
            },
            Entry {
                glyph: "😵",
                png: include_bytes!("../assets/emoji/1f635.png"),
            },
            Entry {
                glyph: "😶",
                png: include_bytes!("../assets/emoji/1f636.png"),
            },
            Entry {
                glyph: "😷",
                png: include_bytes!("../assets/emoji/1f637.png"),
            },
            Entry {
                glyph: "😸",
                png: include_bytes!("../assets/emoji/1f638.png"),
            },
            Entry {
                glyph: "😹",
                png: include_bytes!("../assets/emoji/1f639.png"),
            },
            Entry {
                glyph: "😺",
                png: include_bytes!("../assets/emoji/1f63a.png"),
            },
            Entry {
                glyph: "😻",
                png: include_bytes!("../assets/emoji/1f63b.png"),
            },
            Entry {
                glyph: "😼",
                png: include_bytes!("../assets/emoji/1f63c.png"),
            },
            Entry {
                glyph: "😽",
                png: include_bytes!("../assets/emoji/1f63d.png"),
            },
            Entry {
                glyph: "😾",
                png: include_bytes!("../assets/emoji/1f63e.png"),
            },
            Entry {
                glyph: "😿",
                png: include_bytes!("../assets/emoji/1f63f.png"),
            },
            Entry {
                glyph: "🙀",
                png: include_bytes!("../assets/emoji/1f640.png"),
            },
            Entry {
                glyph: "🙁",
                png: include_bytes!("../assets/emoji/1f641.png"),
            },
            Entry {
                glyph: "🙂",
                png: include_bytes!("../assets/emoji/1f642.png"),
            },
            Entry {
                glyph: "🙃",
                png: include_bytes!("../assets/emoji/1f643.png"),
            },
            Entry {
                glyph: "🙄",
                png: include_bytes!("../assets/emoji/1f644.png"),
            },
            Entry {
                glyph: "🙅",
                png: include_bytes!("../assets/emoji/1f645.png"),
            },
            Entry {
                glyph: "🙆",
                png: include_bytes!("../assets/emoji/1f646.png"),
            },
            Entry {
                glyph: "🙇",
                png: include_bytes!("../assets/emoji/1f647.png"),
            },
            Entry {
                glyph: "🙈",
                png: include_bytes!("../assets/emoji/1f648.png"),
            },
            Entry {
                glyph: "🙉",
                png: include_bytes!("../assets/emoji/1f649.png"),
            },
            Entry {
                glyph: "🙊",
                png: include_bytes!("../assets/emoji/1f64a.png"),
            },
            Entry {
                glyph: "🙋",
                png: include_bytes!("../assets/emoji/1f64b.png"),
            },
            Entry {
                glyph: "🙌",
                png: include_bytes!("../assets/emoji/1f64c.png"),
            },
            Entry {
                glyph: "🙍",
                png: include_bytes!("../assets/emoji/1f64d.png"),
            },
            Entry {
                glyph: "🙎",
                png: include_bytes!("../assets/emoji/1f64e.png"),
            },
            Entry {
                glyph: "🙏",
                png: include_bytes!("../assets/emoji/1f64f.png"),
            },
            Entry {
                glyph: "🤐",
                png: include_bytes!("../assets/emoji/1f910.png"),
            },
            Entry {
                glyph: "🤑",
                png: include_bytes!("../assets/emoji/1f911.png"),
            },
            Entry {
                glyph: "🤒",
                png: include_bytes!("../assets/emoji/1f912.png"),
            },
            Entry {
                glyph: "🤓",
                png: include_bytes!("../assets/emoji/1f913.png"),
            },
            Entry {
                glyph: "🤔",
                png: include_bytes!("../assets/emoji/1f914.png"),
            },
            Entry {
                glyph: "🤕",
                png: include_bytes!("../assets/emoji/1f915.png"),
            },
            Entry {
                glyph: "🤖",
                png: include_bytes!("../assets/emoji/1f916.png"),
            },
            Entry {
                glyph: "🤗",
                png: include_bytes!("../assets/emoji/1f917.png"),
            },
            Entry {
                glyph: "🤘",
                png: include_bytes!("../assets/emoji/1f918.png"),
            },
            Entry {
                glyph: "🤙",
                png: include_bytes!("../assets/emoji/1f919.png"),
            },
            Entry {
                glyph: "🤚",
                png: include_bytes!("../assets/emoji/1f91a.png"),
            },
            Entry {
                glyph: "🤛",
                png: include_bytes!("../assets/emoji/1f91b.png"),
            },
            Entry {
                glyph: "🤜",
                png: include_bytes!("../assets/emoji/1f91c.png"),
            },
            Entry {
                glyph: "🤝",
                png: include_bytes!("../assets/emoji/1f91d.png"),
            },
            Entry {
                glyph: "🤞",
                png: include_bytes!("../assets/emoji/1f91e.png"),
            },
            Entry {
                glyph: "🤟",
                png: include_bytes!("../assets/emoji/1f91f.png"),
            },
            Entry {
                glyph: "🤠",
                png: include_bytes!("../assets/emoji/1f920.png"),
            },
            Entry {
                glyph: "🤡",
                png: include_bytes!("../assets/emoji/1f921.png"),
            },
            Entry {
                glyph: "🤢",
                png: include_bytes!("../assets/emoji/1f922.png"),
            },
            Entry {
                glyph: "🤣",
                png: include_bytes!("../assets/emoji/1f923.png"),
            },
            Entry {
                glyph: "🤤",
                png: include_bytes!("../assets/emoji/1f924.png"),
            },
            Entry {
                glyph: "🤥",
                png: include_bytes!("../assets/emoji/1f925.png"),
            },
            Entry {
                glyph: "🤦",
                png: include_bytes!("../assets/emoji/1f926.png"),
            },
            Entry {
                glyph: "🤧",
                png: include_bytes!("../assets/emoji/1f927.png"),
            },
            Entry {
                glyph: "🤨",
                png: include_bytes!("../assets/emoji/1f928.png"),
            },
            Entry {
                glyph: "🤩",
                png: include_bytes!("../assets/emoji/1f929.png"),
            },
            Entry {
                glyph: "🤪",
                png: include_bytes!("../assets/emoji/1f92a.png"),
            },
            Entry {
                glyph: "🤫",
                png: include_bytes!("../assets/emoji/1f92b.png"),
            },
            Entry {
                glyph: "🤬",
                png: include_bytes!("../assets/emoji/1f92c.png"),
            },
            Entry {
                glyph: "🤭",
                png: include_bytes!("../assets/emoji/1f92d.png"),
            },
            Entry {
                glyph: "🤮",
                png: include_bytes!("../assets/emoji/1f92e.png"),
            },
            Entry {
                glyph: "🤯",
                png: include_bytes!("../assets/emoji/1f92f.png"),
            },
            Entry {
                glyph: "🥰",
                png: include_bytes!("../assets/emoji/1f970.png"),
            },
            Entry {
                glyph: "🥱",
                png: include_bytes!("../assets/emoji/1f971.png"),
            },
            Entry {
                glyph: "🥲",
                png: include_bytes!("../assets/emoji/1f972.png"),
            },
            Entry {
                glyph: "🥳",
                png: include_bytes!("../assets/emoji/1f973.png"),
            },
            Entry {
                glyph: "🥴",
                png: include_bytes!("../assets/emoji/1f974.png"),
            },
            Entry {
                glyph: "🥵",
                png: include_bytes!("../assets/emoji/1f975.png"),
            },
            Entry {
                glyph: "🥶",
                png: include_bytes!("../assets/emoji/1f976.png"),
            },
            Entry {
                glyph: "🥷",
                png: include_bytes!("../assets/emoji/1f977.png"),
            },
            Entry {
                glyph: "🥸",
                png: include_bytes!("../assets/emoji/1f978.png"),
            },
            Entry {
                glyph: "🥹",
                png: include_bytes!("../assets/emoji/1f979.png"),
            },
            Entry {
                glyph: "🥺",
                png: include_bytes!("../assets/emoji/1f97a.png"),
            },
            Entry {
                glyph: "🧐",
                png: include_bytes!("../assets/emoji/1f9d0.png"),
            },
            Entry {
                glyph: "☹️",
                png: include_bytes!("../assets/emoji/2639.png"),
            },
            Entry {
                glyph: "☺️",
                png: include_bytes!("../assets/emoji/263a.png"),
            },
        ],
    ),
    (
        "Люди",
        "👍",
        &[
            Entry {
                glyph: "👀",
                png: include_bytes!("../assets/emoji/1f440.png"),
            },
            Entry {
                glyph: "👁️",
                png: include_bytes!("../assets/emoji/1f441.png"),
            },
            Entry {
                glyph: "👂",
                png: include_bytes!("../assets/emoji/1f442.png"),
            },
            Entry {
                glyph: "👃",
                png: include_bytes!("../assets/emoji/1f443.png"),
            },
            Entry {
                glyph: "👄",
                png: include_bytes!("../assets/emoji/1f444.png"),
            },
            Entry {
                glyph: "👅",
                png: include_bytes!("../assets/emoji/1f445.png"),
            },
            Entry {
                glyph: "👆",
                png: include_bytes!("../assets/emoji/1f446.png"),
            },
            Entry {
                glyph: "👇",
                png: include_bytes!("../assets/emoji/1f447.png"),
            },
            Entry {
                glyph: "👈",
                png: include_bytes!("../assets/emoji/1f448.png"),
            },
            Entry {
                glyph: "👉",
                png: include_bytes!("../assets/emoji/1f449.png"),
            },
            Entry {
                glyph: "👊",
                png: include_bytes!("../assets/emoji/1f44a.png"),
            },
            Entry {
                glyph: "👋",
                png: include_bytes!("../assets/emoji/1f44b.png"),
            },
            Entry {
                glyph: "👌",
                png: include_bytes!("../assets/emoji/1f44c.png"),
            },
            Entry {
                glyph: "👍",
                png: include_bytes!("../assets/emoji/1f44d.png"),
            },
            Entry {
                glyph: "👎",
                png: include_bytes!("../assets/emoji/1f44e.png"),
            },
            Entry {
                glyph: "👏",
                png: include_bytes!("../assets/emoji/1f44f.png"),
            },
            Entry {
                glyph: "👐",
                png: include_bytes!("../assets/emoji/1f450.png"),
            },
            Entry {
                glyph: "👑",
                png: include_bytes!("../assets/emoji/1f451.png"),
            },
            Entry {
                glyph: "👒",
                png: include_bytes!("../assets/emoji/1f452.png"),
            },
            Entry {
                glyph: "👓",
                png: include_bytes!("../assets/emoji/1f453.png"),
            },
            Entry {
                glyph: "👔",
                png: include_bytes!("../assets/emoji/1f454.png"),
            },
            Entry {
                glyph: "👕",
                png: include_bytes!("../assets/emoji/1f455.png"),
            },
            Entry {
                glyph: "👖",
                png: include_bytes!("../assets/emoji/1f456.png"),
            },
            Entry {
                glyph: "👗",
                png: include_bytes!("../assets/emoji/1f457.png"),
            },
            Entry {
                glyph: "👘",
                png: include_bytes!("../assets/emoji/1f458.png"),
            },
            Entry {
                glyph: "👙",
                png: include_bytes!("../assets/emoji/1f459.png"),
            },
            Entry {
                glyph: "👚",
                png: include_bytes!("../assets/emoji/1f45a.png"),
            },
            Entry {
                glyph: "👛",
                png: include_bytes!("../assets/emoji/1f45b.png"),
            },
            Entry {
                glyph: "👜",
                png: include_bytes!("../assets/emoji/1f45c.png"),
            },
            Entry {
                glyph: "👝",
                png: include_bytes!("../assets/emoji/1f45d.png"),
            },
            Entry {
                glyph: "👞",
                png: include_bytes!("../assets/emoji/1f45e.png"),
            },
            Entry {
                glyph: "👟",
                png: include_bytes!("../assets/emoji/1f45f.png"),
            },
            Entry {
                glyph: "👠",
                png: include_bytes!("../assets/emoji/1f460.png"),
            },
            Entry {
                glyph: "👡",
                png: include_bytes!("../assets/emoji/1f461.png"),
            },
            Entry {
                glyph: "👢",
                png: include_bytes!("../assets/emoji/1f462.png"),
            },
            Entry {
                glyph: "👣",
                png: include_bytes!("../assets/emoji/1f463.png"),
            },
            Entry {
                glyph: "👤",
                png: include_bytes!("../assets/emoji/1f464.png"),
            },
            Entry {
                glyph: "👥",
                png: include_bytes!("../assets/emoji/1f465.png"),
            },
            Entry {
                glyph: "👦",
                png: include_bytes!("../assets/emoji/1f466.png"),
            },
            Entry {
                glyph: "👧",
                png: include_bytes!("../assets/emoji/1f467.png"),
            },
            Entry {
                glyph: "👨",
                png: include_bytes!("../assets/emoji/1f468.png"),
            },
            Entry {
                glyph: "👩",
                png: include_bytes!("../assets/emoji/1f469.png"),
            },
            Entry {
                glyph: "👪",
                png: include_bytes!("../assets/emoji/1f46a.png"),
            },
            Entry {
                glyph: "👫",
                png: include_bytes!("../assets/emoji/1f46b.png"),
            },
            Entry {
                glyph: "👬",
                png: include_bytes!("../assets/emoji/1f46c.png"),
            },
            Entry {
                glyph: "👭",
                png: include_bytes!("../assets/emoji/1f46d.png"),
            },
            Entry {
                glyph: "👮",
                png: include_bytes!("../assets/emoji/1f46e.png"),
            },
            Entry {
                glyph: "👯",
                png: include_bytes!("../assets/emoji/1f46f.png"),
            },
            Entry {
                glyph: "👰",
                png: include_bytes!("../assets/emoji/1f470.png"),
            },
            Entry {
                glyph: "👱",
                png: include_bytes!("../assets/emoji/1f471.png"),
            },
            Entry {
                glyph: "👲",
                png: include_bytes!("../assets/emoji/1f472.png"),
            },
            Entry {
                glyph: "👳",
                png: include_bytes!("../assets/emoji/1f473.png"),
            },
            Entry {
                glyph: "👴",
                png: include_bytes!("../assets/emoji/1f474.png"),
            },
            Entry {
                glyph: "👵",
                png: include_bytes!("../assets/emoji/1f475.png"),
            },
            Entry {
                glyph: "👶",
                png: include_bytes!("../assets/emoji/1f476.png"),
            },
            Entry {
                glyph: "👷",
                png: include_bytes!("../assets/emoji/1f477.png"),
            },
            Entry {
                glyph: "👸",
                png: include_bytes!("../assets/emoji/1f478.png"),
            },
            Entry {
                glyph: "👹",
                png: include_bytes!("../assets/emoji/1f479.png"),
            },
            Entry {
                glyph: "👺",
                png: include_bytes!("../assets/emoji/1f47a.png"),
            },
            Entry {
                glyph: "👻",
                png: include_bytes!("../assets/emoji/1f47b.png"),
            },
            Entry {
                glyph: "👼",
                png: include_bytes!("../assets/emoji/1f47c.png"),
            },
            Entry {
                glyph: "👽",
                png: include_bytes!("../assets/emoji/1f47d.png"),
            },
            Entry {
                glyph: "👾",
                png: include_bytes!("../assets/emoji/1f47e.png"),
            },
            Entry {
                glyph: "👿",
                png: include_bytes!("../assets/emoji/1f47f.png"),
            },
            Entry {
                glyph: "💀",
                png: include_bytes!("../assets/emoji/1f480.png"),
            },
            Entry {
                glyph: "💁",
                png: include_bytes!("../assets/emoji/1f481.png"),
            },
            Entry {
                glyph: "💂",
                png: include_bytes!("../assets/emoji/1f482.png"),
            },
            Entry {
                glyph: "💃",
                png: include_bytes!("../assets/emoji/1f483.png"),
            },
            Entry {
                glyph: "💄",
                png: include_bytes!("../assets/emoji/1f484.png"),
            },
            Entry {
                glyph: "💅",
                png: include_bytes!("../assets/emoji/1f485.png"),
            },
            Entry {
                glyph: "💆",
                png: include_bytes!("../assets/emoji/1f486.png"),
            },
            Entry {
                glyph: "💇",
                png: include_bytes!("../assets/emoji/1f487.png"),
            },
            Entry {
                glyph: "💪",
                png: include_bytes!("../assets/emoji/1f4aa.png"),
            },
            Entry {
                glyph: "🤌",
                png: include_bytes!("../assets/emoji/1f90c.png"),
            },
            Entry {
                glyph: "🤍",
                png: include_bytes!("../assets/emoji/1f90d.png"),
            },
            Entry {
                glyph: "🤎",
                png: include_bytes!("../assets/emoji/1f90e.png"),
            },
            Entry {
                glyph: "🤏",
                png: include_bytes!("../assets/emoji/1f90f.png"),
            },
            Entry {
                glyph: "🤰",
                png: include_bytes!("../assets/emoji/1f930.png"),
            },
            Entry {
                glyph: "🤱",
                png: include_bytes!("../assets/emoji/1f931.png"),
            },
            Entry {
                glyph: "🤲",
                png: include_bytes!("../assets/emoji/1f932.png"),
            },
            Entry {
                glyph: "🤳",
                png: include_bytes!("../assets/emoji/1f933.png"),
            },
            Entry {
                glyph: "🤴",
                png: include_bytes!("../assets/emoji/1f934.png"),
            },
            Entry {
                glyph: "🤵",
                png: include_bytes!("../assets/emoji/1f935.png"),
            },
            Entry {
                glyph: "🤶",
                png: include_bytes!("../assets/emoji/1f936.png"),
            },
            Entry {
                glyph: "🤷",
                png: include_bytes!("../assets/emoji/1f937.png"),
            },
            Entry {
                glyph: "🤸",
                png: include_bytes!("../assets/emoji/1f938.png"),
            },
            Entry {
                glyph: "🤹",
                png: include_bytes!("../assets/emoji/1f939.png"),
            },
            Entry {
                glyph: "🤺",
                png: include_bytes!("../assets/emoji/1f93a.png"),
            },
            Entry {
                glyph: "🦴",
                png: include_bytes!("../assets/emoji/1f9b4.png"),
            },
            Entry {
                glyph: "🦵",
                png: include_bytes!("../assets/emoji/1f9b5.png"),
            },
            Entry {
                glyph: "🦶",
                png: include_bytes!("../assets/emoji/1f9b6.png"),
            },
            Entry {
                glyph: "🦷",
                png: include_bytes!("../assets/emoji/1f9b7.png"),
            },
            Entry {
                glyph: "🦸",
                png: include_bytes!("../assets/emoji/1f9b8.png"),
            },
            Entry {
                glyph: "🦹",
                png: include_bytes!("../assets/emoji/1f9b9.png"),
            },
            Entry {
                glyph: "🦺",
                png: include_bytes!("../assets/emoji/1f9ba.png"),
            },
            Entry {
                glyph: "🦻",
                png: include_bytes!("../assets/emoji/1f9bb.png"),
            },
            Entry {
                glyph: "🧑",
                png: include_bytes!("../assets/emoji/1f9d1.png"),
            },
            Entry {
                glyph: "🧒",
                png: include_bytes!("../assets/emoji/1f9d2.png"),
            },
            Entry {
                glyph: "🧓",
                png: include_bytes!("../assets/emoji/1f9d3.png"),
            },
            Entry {
                glyph: "🧔",
                png: include_bytes!("../assets/emoji/1f9d4.png"),
            },
            Entry {
                glyph: "🧕",
                png: include_bytes!("../assets/emoji/1f9d5.png"),
            },
            Entry {
                glyph: "🧖",
                png: include_bytes!("../assets/emoji/1f9d6.png"),
            },
            Entry {
                glyph: "🧗",
                png: include_bytes!("../assets/emoji/1f9d7.png"),
            },
            Entry {
                glyph: "🧘",
                png: include_bytes!("../assets/emoji/1f9d8.png"),
            },
            Entry {
                glyph: "🧙",
                png: include_bytes!("../assets/emoji/1f9d9.png"),
            },
            Entry {
                glyph: "🧚",
                png: include_bytes!("../assets/emoji/1f9da.png"),
            },
            Entry {
                glyph: "🧛",
                png: include_bytes!("../assets/emoji/1f9db.png"),
            },
            Entry {
                glyph: "🧜",
                png: include_bytes!("../assets/emoji/1f9dc.png"),
            },
            Entry {
                glyph: "🧝",
                png: include_bytes!("../assets/emoji/1f9dd.png"),
            },
            Entry {
                glyph: "✊",
                png: include_bytes!("../assets/emoji/270a.png"),
            },
            Entry {
                glyph: "✋",
                png: include_bytes!("../assets/emoji/270b.png"),
            },
            Entry {
                glyph: "✌️",
                png: include_bytes!("../assets/emoji/270c.png"),
            },
            Entry {
                glyph: "✍️",
                png: include_bytes!("../assets/emoji/270d.png"),
            },
            Entry {
                glyph: "☝️",
                png: include_bytes!("../assets/emoji/261d.png"),
            },
        ],
    ),
    (
        "Природа",
        "🐶",
        &[
            Entry {
                glyph: "🐀",
                png: include_bytes!("../assets/emoji/1f400.png"),
            },
            Entry {
                glyph: "🐁",
                png: include_bytes!("../assets/emoji/1f401.png"),
            },
            Entry {
                glyph: "🐂",
                png: include_bytes!("../assets/emoji/1f402.png"),
            },
            Entry {
                glyph: "🐃",
                png: include_bytes!("../assets/emoji/1f403.png"),
            },
            Entry {
                glyph: "🐄",
                png: include_bytes!("../assets/emoji/1f404.png"),
            },
            Entry {
                glyph: "🐅",
                png: include_bytes!("../assets/emoji/1f405.png"),
            },
            Entry {
                glyph: "🐆",
                png: include_bytes!("../assets/emoji/1f406.png"),
            },
            Entry {
                glyph: "🐇",
                png: include_bytes!("../assets/emoji/1f407.png"),
            },
            Entry {
                glyph: "🐈",
                png: include_bytes!("../assets/emoji/1f408.png"),
            },
            Entry {
                glyph: "🐉",
                png: include_bytes!("../assets/emoji/1f409.png"),
            },
            Entry {
                glyph: "🐊",
                png: include_bytes!("../assets/emoji/1f40a.png"),
            },
            Entry {
                glyph: "🐋",
                png: include_bytes!("../assets/emoji/1f40b.png"),
            },
            Entry {
                glyph: "🐌",
                png: include_bytes!("../assets/emoji/1f40c.png"),
            },
            Entry {
                glyph: "🐍",
                png: include_bytes!("../assets/emoji/1f40d.png"),
            },
            Entry {
                glyph: "🐎",
                png: include_bytes!("../assets/emoji/1f40e.png"),
            },
            Entry {
                glyph: "🐏",
                png: include_bytes!("../assets/emoji/1f40f.png"),
            },
            Entry {
                glyph: "🐐",
                png: include_bytes!("../assets/emoji/1f410.png"),
            },
            Entry {
                glyph: "🐑",
                png: include_bytes!("../assets/emoji/1f411.png"),
            },
            Entry {
                glyph: "🐒",
                png: include_bytes!("../assets/emoji/1f412.png"),
            },
            Entry {
                glyph: "🐓",
                png: include_bytes!("../assets/emoji/1f413.png"),
            },
            Entry {
                glyph: "🐔",
                png: include_bytes!("../assets/emoji/1f414.png"),
            },
            Entry {
                glyph: "🐕",
                png: include_bytes!("../assets/emoji/1f415.png"),
            },
            Entry {
                glyph: "🐖",
                png: include_bytes!("../assets/emoji/1f416.png"),
            },
            Entry {
                glyph: "🐗",
                png: include_bytes!("../assets/emoji/1f417.png"),
            },
            Entry {
                glyph: "🐘",
                png: include_bytes!("../assets/emoji/1f418.png"),
            },
            Entry {
                glyph: "🐙",
                png: include_bytes!("../assets/emoji/1f419.png"),
            },
            Entry {
                glyph: "🐚",
                png: include_bytes!("../assets/emoji/1f41a.png"),
            },
            Entry {
                glyph: "🐛",
                png: include_bytes!("../assets/emoji/1f41b.png"),
            },
            Entry {
                glyph: "🐜",
                png: include_bytes!("../assets/emoji/1f41c.png"),
            },
            Entry {
                glyph: "🐝",
                png: include_bytes!("../assets/emoji/1f41d.png"),
            },
            Entry {
                glyph: "🐞",
                png: include_bytes!("../assets/emoji/1f41e.png"),
            },
            Entry {
                glyph: "🐟",
                png: include_bytes!("../assets/emoji/1f41f.png"),
            },
            Entry {
                glyph: "🐠",
                png: include_bytes!("../assets/emoji/1f420.png"),
            },
            Entry {
                glyph: "🐡",
                png: include_bytes!("../assets/emoji/1f421.png"),
            },
            Entry {
                glyph: "🐢",
                png: include_bytes!("../assets/emoji/1f422.png"),
            },
            Entry {
                glyph: "🐣",
                png: include_bytes!("../assets/emoji/1f423.png"),
            },
            Entry {
                glyph: "🐤",
                png: include_bytes!("../assets/emoji/1f424.png"),
            },
            Entry {
                glyph: "🐥",
                png: include_bytes!("../assets/emoji/1f425.png"),
            },
            Entry {
                glyph: "🐦",
                png: include_bytes!("../assets/emoji/1f426.png"),
            },
            Entry {
                glyph: "🐧",
                png: include_bytes!("../assets/emoji/1f427.png"),
            },
            Entry {
                glyph: "🐨",
                png: include_bytes!("../assets/emoji/1f428.png"),
            },
            Entry {
                glyph: "🐩",
                png: include_bytes!("../assets/emoji/1f429.png"),
            },
            Entry {
                glyph: "🐪",
                png: include_bytes!("../assets/emoji/1f42a.png"),
            },
            Entry {
                glyph: "🐫",
                png: include_bytes!("../assets/emoji/1f42b.png"),
            },
            Entry {
                glyph: "🐬",
                png: include_bytes!("../assets/emoji/1f42c.png"),
            },
            Entry {
                glyph: "🐭",
                png: include_bytes!("../assets/emoji/1f42d.png"),
            },
            Entry {
                glyph: "🐮",
                png: include_bytes!("../assets/emoji/1f42e.png"),
            },
            Entry {
                glyph: "🐯",
                png: include_bytes!("../assets/emoji/1f42f.png"),
            },
            Entry {
                glyph: "🐰",
                png: include_bytes!("../assets/emoji/1f430.png"),
            },
            Entry {
                glyph: "🐱",
                png: include_bytes!("../assets/emoji/1f431.png"),
            },
            Entry {
                glyph: "🐲",
                png: include_bytes!("../assets/emoji/1f432.png"),
            },
            Entry {
                glyph: "🐳",
                png: include_bytes!("../assets/emoji/1f433.png"),
            },
            Entry {
                glyph: "🐴",
                png: include_bytes!("../assets/emoji/1f434.png"),
            },
            Entry {
                glyph: "🐵",
                png: include_bytes!("../assets/emoji/1f435.png"),
            },
            Entry {
                glyph: "🐶",
                png: include_bytes!("../assets/emoji/1f436.png"),
            },
            Entry {
                glyph: "🐷",
                png: include_bytes!("../assets/emoji/1f437.png"),
            },
            Entry {
                glyph: "🐸",
                png: include_bytes!("../assets/emoji/1f438.png"),
            },
            Entry {
                glyph: "🐹",
                png: include_bytes!("../assets/emoji/1f439.png"),
            },
            Entry {
                glyph: "🐺",
                png: include_bytes!("../assets/emoji/1f43a.png"),
            },
            Entry {
                glyph: "🐻",
                png: include_bytes!("../assets/emoji/1f43b.png"),
            },
            Entry {
                glyph: "🐼",
                png: include_bytes!("../assets/emoji/1f43c.png"),
            },
            Entry {
                glyph: "🐽",
                png: include_bytes!("../assets/emoji/1f43d.png"),
            },
            Entry {
                glyph: "🐾",
                png: include_bytes!("../assets/emoji/1f43e.png"),
            },
            Entry {
                glyph: "🐿️",
                png: include_bytes!("../assets/emoji/1f43f.png"),
            },
            Entry {
                glyph: "🦀",
                png: include_bytes!("../assets/emoji/1f980.png"),
            },
            Entry {
                glyph: "🦁",
                png: include_bytes!("../assets/emoji/1f981.png"),
            },
            Entry {
                glyph: "🦂",
                png: include_bytes!("../assets/emoji/1f982.png"),
            },
            Entry {
                glyph: "🦃",
                png: include_bytes!("../assets/emoji/1f983.png"),
            },
            Entry {
                glyph: "🦄",
                png: include_bytes!("../assets/emoji/1f984.png"),
            },
            Entry {
                glyph: "🦅",
                png: include_bytes!("../assets/emoji/1f985.png"),
            },
            Entry {
                glyph: "🦆",
                png: include_bytes!("../assets/emoji/1f986.png"),
            },
            Entry {
                glyph: "🦇",
                png: include_bytes!("../assets/emoji/1f987.png"),
            },
            Entry {
                glyph: "🦈",
                png: include_bytes!("../assets/emoji/1f988.png"),
            },
            Entry {
                glyph: "🦉",
                png: include_bytes!("../assets/emoji/1f989.png"),
            },
            Entry {
                glyph: "🦊",
                png: include_bytes!("../assets/emoji/1f98a.png"),
            },
            Entry {
                glyph: "🦋",
                png: include_bytes!("../assets/emoji/1f98b.png"),
            },
            Entry {
                glyph: "🦌",
                png: include_bytes!("../assets/emoji/1f98c.png"),
            },
            Entry {
                glyph: "🦍",
                png: include_bytes!("../assets/emoji/1f98d.png"),
            },
            Entry {
                glyph: "🦎",
                png: include_bytes!("../assets/emoji/1f98e.png"),
            },
            Entry {
                glyph: "🦏",
                png: include_bytes!("../assets/emoji/1f98f.png"),
            },
            Entry {
                glyph: "🦐",
                png: include_bytes!("../assets/emoji/1f990.png"),
            },
            Entry {
                glyph: "🦑",
                png: include_bytes!("../assets/emoji/1f991.png"),
            },
            Entry {
                glyph: "🦒",
                png: include_bytes!("../assets/emoji/1f992.png"),
            },
            Entry {
                glyph: "🦓",
                png: include_bytes!("../assets/emoji/1f993.png"),
            },
            Entry {
                glyph: "🦔",
                png: include_bytes!("../assets/emoji/1f994.png"),
            },
            Entry {
                glyph: "🦕",
                png: include_bytes!("../assets/emoji/1f995.png"),
            },
            Entry {
                glyph: "🦖",
                png: include_bytes!("../assets/emoji/1f996.png"),
            },
            Entry {
                glyph: "🦗",
                png: include_bytes!("../assets/emoji/1f997.png"),
            },
            Entry {
                glyph: "🦘",
                png: include_bytes!("../assets/emoji/1f998.png"),
            },
            Entry {
                glyph: "🦙",
                png: include_bytes!("../assets/emoji/1f999.png"),
            },
            Entry {
                glyph: "🦚",
                png: include_bytes!("../assets/emoji/1f99a.png"),
            },
            Entry {
                glyph: "🦛",
                png: include_bytes!("../assets/emoji/1f99b.png"),
            },
            Entry {
                glyph: "🦜",
                png: include_bytes!("../assets/emoji/1f99c.png"),
            },
            Entry {
                glyph: "🦝",
                png: include_bytes!("../assets/emoji/1f99d.png"),
            },
            Entry {
                glyph: "🦞",
                png: include_bytes!("../assets/emoji/1f99e.png"),
            },
            Entry {
                glyph: "🦟",
                png: include_bytes!("../assets/emoji/1f99f.png"),
            },
            Entry {
                glyph: "🦠",
                png: include_bytes!("../assets/emoji/1f9a0.png"),
            },
            Entry {
                glyph: "🦡",
                png: include_bytes!("../assets/emoji/1f9a1.png"),
            },
            Entry {
                glyph: "🦢",
                png: include_bytes!("../assets/emoji/1f9a2.png"),
            },
            Entry {
                glyph: "🦣",
                png: include_bytes!("../assets/emoji/1f9a3.png"),
            },
            Entry {
                glyph: "🦤",
                png: include_bytes!("../assets/emoji/1f9a4.png"),
            },
            Entry {
                glyph: "🦥",
                png: include_bytes!("../assets/emoji/1f9a5.png"),
            },
            Entry {
                glyph: "🦦",
                png: include_bytes!("../assets/emoji/1f9a6.png"),
            },
            Entry {
                glyph: "🦧",
                png: include_bytes!("../assets/emoji/1f9a7.png"),
            },
            Entry {
                glyph: "🦨",
                png: include_bytes!("../assets/emoji/1f9a8.png"),
            },
            Entry {
                glyph: "🦩",
                png: include_bytes!("../assets/emoji/1f9a9.png"),
            },
            Entry {
                glyph: "🦪",
                png: include_bytes!("../assets/emoji/1f9aa.png"),
            },
            Entry {
                glyph: "🦫",
                png: include_bytes!("../assets/emoji/1f9ab.png"),
            },
            Entry {
                glyph: "🦬",
                png: include_bytes!("../assets/emoji/1f9ac.png"),
            },
            Entry {
                glyph: "🦭",
                png: include_bytes!("../assets/emoji/1f9ad.png"),
            },
            Entry {
                glyph: "🦮",
                png: include_bytes!("../assets/emoji/1f9ae.png"),
            },
            Entry {
                glyph: "🌰",
                png: include_bytes!("../assets/emoji/1f330.png"),
            },
            Entry {
                glyph: "🌱",
                png: include_bytes!("../assets/emoji/1f331.png"),
            },
            Entry {
                glyph: "🌲",
                png: include_bytes!("../assets/emoji/1f332.png"),
            },
            Entry {
                glyph: "🌳",
                png: include_bytes!("../assets/emoji/1f333.png"),
            },
            Entry {
                glyph: "🌴",
                png: include_bytes!("../assets/emoji/1f334.png"),
            },
            Entry {
                glyph: "🌵",
                png: include_bytes!("../assets/emoji/1f335.png"),
            },
            Entry {
                glyph: "🌶️",
                png: include_bytes!("../assets/emoji/1f336.png"),
            },
            Entry {
                glyph: "🌷",
                png: include_bytes!("../assets/emoji/1f337.png"),
            },
            Entry {
                glyph: "🌸",
                png: include_bytes!("../assets/emoji/1f338.png"),
            },
            Entry {
                glyph: "🌹",
                png: include_bytes!("../assets/emoji/1f339.png"),
            },
            Entry {
                glyph: "🌺",
                png: include_bytes!("../assets/emoji/1f33a.png"),
            },
            Entry {
                glyph: "🌻",
                png: include_bytes!("../assets/emoji/1f33b.png"),
            },
            Entry {
                glyph: "🌼",
                png: include_bytes!("../assets/emoji/1f33c.png"),
            },
            Entry {
                glyph: "🌽",
                png: include_bytes!("../assets/emoji/1f33d.png"),
            },
            Entry {
                glyph: "🌾",
                png: include_bytes!("../assets/emoji/1f33e.png"),
            },
            Entry {
                glyph: "🌿",
                png: include_bytes!("../assets/emoji/1f33f.png"),
            },
            Entry {
                glyph: "🍀",
                png: include_bytes!("../assets/emoji/1f340.png"),
            },
            Entry {
                glyph: "🍁",
                png: include_bytes!("../assets/emoji/1f341.png"),
            },
            Entry {
                glyph: "🍂",
                png: include_bytes!("../assets/emoji/1f342.png"),
            },
            Entry {
                glyph: "🍃",
                png: include_bytes!("../assets/emoji/1f343.png"),
            },
            Entry {
                glyph: "🍄",
                png: include_bytes!("../assets/emoji/1f344.png"),
            },
            Entry {
                glyph: "💐",
                png: include_bytes!("../assets/emoji/1f490.png"),
            },
            Entry {
                glyph: "☀️",
                png: include_bytes!("../assets/emoji/2600.png"),
            },
            Entry {
                glyph: "☁️",
                png: include_bytes!("../assets/emoji/2601.png"),
            },
            Entry {
                glyph: "☂️",
                png: include_bytes!("../assets/emoji/2602.png"),
            },
            Entry {
                glyph: "☃️",
                png: include_bytes!("../assets/emoji/2603.png"),
            },
            Entry {
                glyph: "☄️",
                png: include_bytes!("../assets/emoji/2604.png"),
            },
            Entry {
                glyph: "⚡",
                png: include_bytes!("../assets/emoji/26a1.png"),
            },
            Entry {
                glyph: "❄️",
                png: include_bytes!("../assets/emoji/2744.png"),
            },
            Entry {
                glyph: "🌈",
                png: include_bytes!("../assets/emoji/1f308.png"),
            },
            Entry {
                glyph: "🌉",
                png: include_bytes!("../assets/emoji/1f309.png"),
            },
            Entry {
                glyph: "🌊",
                png: include_bytes!("../assets/emoji/1f30a.png"),
            },
            Entry {
                glyph: "🌋",
                png: include_bytes!("../assets/emoji/1f30b.png"),
            },
            Entry {
                glyph: "🌌",
                png: include_bytes!("../assets/emoji/1f30c.png"),
            },
            Entry {
                glyph: "🌍",
                png: include_bytes!("../assets/emoji/1f30d.png"),
            },
            Entry {
                glyph: "🌎",
                png: include_bytes!("../assets/emoji/1f30e.png"),
            },
            Entry {
                glyph: "🌏",
                png: include_bytes!("../assets/emoji/1f30f.png"),
            },
            Entry {
                glyph: "🌙",
                png: include_bytes!("../assets/emoji/1f319.png"),
            },
            Entry {
                glyph: "🌚",
                png: include_bytes!("../assets/emoji/1f31a.png"),
            },
            Entry {
                glyph: "🌛",
                png: include_bytes!("../assets/emoji/1f31b.png"),
            },
            Entry {
                glyph: "🌜",
                png: include_bytes!("../assets/emoji/1f31c.png"),
            },
            Entry {
                glyph: "🌝",
                png: include_bytes!("../assets/emoji/1f31d.png"),
            },
            Entry {
                glyph: "🌞",
                png: include_bytes!("../assets/emoji/1f31e.png"),
            },
            Entry {
                glyph: "🌟",
                png: include_bytes!("../assets/emoji/1f31f.png"),
            },
            Entry {
                glyph: "🌠",
                png: include_bytes!("../assets/emoji/1f320.png"),
            },
            Entry {
                glyph: "🌡️",
                png: include_bytes!("../assets/emoji/1f321.png"),
            },
            Entry {
                glyph: "🌪️",
                png: include_bytes!("../assets/emoji/1f32a.png"),
            },
            Entry {
                glyph: "🌫️",
                png: include_bytes!("../assets/emoji/1f32b.png"),
            },
            Entry {
                glyph: "🌬️",
                png: include_bytes!("../assets/emoji/1f32c.png"),
            },
        ],
    ),
    (
        "Еда",
        "🍔",
        &[
            Entry {
                glyph: "🍅",
                png: include_bytes!("../assets/emoji/1f345.png"),
            },
            Entry {
                glyph: "🍆",
                png: include_bytes!("../assets/emoji/1f346.png"),
            },
            Entry {
                glyph: "🍇",
                png: include_bytes!("../assets/emoji/1f347.png"),
            },
            Entry {
                glyph: "🍈",
                png: include_bytes!("../assets/emoji/1f348.png"),
            },
            Entry {
                glyph: "🍉",
                png: include_bytes!("../assets/emoji/1f349.png"),
            },
            Entry {
                glyph: "🍊",
                png: include_bytes!("../assets/emoji/1f34a.png"),
            },
            Entry {
                glyph: "🍋",
                png: include_bytes!("../assets/emoji/1f34b.png"),
            },
            Entry {
                glyph: "🍌",
                png: include_bytes!("../assets/emoji/1f34c.png"),
            },
            Entry {
                glyph: "🍍",
                png: include_bytes!("../assets/emoji/1f34d.png"),
            },
            Entry {
                glyph: "🍎",
                png: include_bytes!("../assets/emoji/1f34e.png"),
            },
            Entry {
                glyph: "🍏",
                png: include_bytes!("../assets/emoji/1f34f.png"),
            },
            Entry {
                glyph: "🍐",
                png: include_bytes!("../assets/emoji/1f350.png"),
            },
            Entry {
                glyph: "🍑",
                png: include_bytes!("../assets/emoji/1f351.png"),
            },
            Entry {
                glyph: "🍒",
                png: include_bytes!("../assets/emoji/1f352.png"),
            },
            Entry {
                glyph: "🍓",
                png: include_bytes!("../assets/emoji/1f353.png"),
            },
            Entry {
                glyph: "🍔",
                png: include_bytes!("../assets/emoji/1f354.png"),
            },
            Entry {
                glyph: "🍕",
                png: include_bytes!("../assets/emoji/1f355.png"),
            },
            Entry {
                glyph: "🍖",
                png: include_bytes!("../assets/emoji/1f356.png"),
            },
            Entry {
                glyph: "🍗",
                png: include_bytes!("../assets/emoji/1f357.png"),
            },
            Entry {
                glyph: "🍘",
                png: include_bytes!("../assets/emoji/1f358.png"),
            },
            Entry {
                glyph: "🍙",
                png: include_bytes!("../assets/emoji/1f359.png"),
            },
            Entry {
                glyph: "🍚",
                png: include_bytes!("../assets/emoji/1f35a.png"),
            },
            Entry {
                glyph: "🍛",
                png: include_bytes!("../assets/emoji/1f35b.png"),
            },
            Entry {
                glyph: "🍜",
                png: include_bytes!("../assets/emoji/1f35c.png"),
            },
            Entry {
                glyph: "🍝",
                png: include_bytes!("../assets/emoji/1f35d.png"),
            },
            Entry {
                glyph: "🍞",
                png: include_bytes!("../assets/emoji/1f35e.png"),
            },
            Entry {
                glyph: "🍟",
                png: include_bytes!("../assets/emoji/1f35f.png"),
            },
            Entry {
                glyph: "🍠",
                png: include_bytes!("../assets/emoji/1f360.png"),
            },
            Entry {
                glyph: "🍡",
                png: include_bytes!("../assets/emoji/1f361.png"),
            },
            Entry {
                glyph: "🍢",
                png: include_bytes!("../assets/emoji/1f362.png"),
            },
            Entry {
                glyph: "🍣",
                png: include_bytes!("../assets/emoji/1f363.png"),
            },
            Entry {
                glyph: "🍤",
                png: include_bytes!("../assets/emoji/1f364.png"),
            },
            Entry {
                glyph: "🍥",
                png: include_bytes!("../assets/emoji/1f365.png"),
            },
            Entry {
                glyph: "🍦",
                png: include_bytes!("../assets/emoji/1f366.png"),
            },
            Entry {
                glyph: "🍧",
                png: include_bytes!("../assets/emoji/1f367.png"),
            },
            Entry {
                glyph: "🍨",
                png: include_bytes!("../assets/emoji/1f368.png"),
            },
            Entry {
                glyph: "🍩",
                png: include_bytes!("../assets/emoji/1f369.png"),
            },
            Entry {
                glyph: "🍪",
                png: include_bytes!("../assets/emoji/1f36a.png"),
            },
            Entry {
                glyph: "🍫",
                png: include_bytes!("../assets/emoji/1f36b.png"),
            },
            Entry {
                glyph: "🍬",
                png: include_bytes!("../assets/emoji/1f36c.png"),
            },
            Entry {
                glyph: "🍭",
                png: include_bytes!("../assets/emoji/1f36d.png"),
            },
            Entry {
                glyph: "🍮",
                png: include_bytes!("../assets/emoji/1f36e.png"),
            },
            Entry {
                glyph: "🍯",
                png: include_bytes!("../assets/emoji/1f36f.png"),
            },
            Entry {
                glyph: "🍰",
                png: include_bytes!("../assets/emoji/1f370.png"),
            },
            Entry {
                glyph: "🍱",
                png: include_bytes!("../assets/emoji/1f371.png"),
            },
            Entry {
                glyph: "🍲",
                png: include_bytes!("../assets/emoji/1f372.png"),
            },
            Entry {
                glyph: "🍳",
                png: include_bytes!("../assets/emoji/1f373.png"),
            },
            Entry {
                glyph: "🍴",
                png: include_bytes!("../assets/emoji/1f374.png"),
            },
            Entry {
                glyph: "🍵",
                png: include_bytes!("../assets/emoji/1f375.png"),
            },
            Entry {
                glyph: "🍶",
                png: include_bytes!("../assets/emoji/1f376.png"),
            },
            Entry {
                glyph: "🍷",
                png: include_bytes!("../assets/emoji/1f377.png"),
            },
            Entry {
                glyph: "🍸",
                png: include_bytes!("../assets/emoji/1f378.png"),
            },
            Entry {
                glyph: "🍹",
                png: include_bytes!("../assets/emoji/1f379.png"),
            },
            Entry {
                glyph: "🍺",
                png: include_bytes!("../assets/emoji/1f37a.png"),
            },
            Entry {
                glyph: "🍻",
                png: include_bytes!("../assets/emoji/1f37b.png"),
            },
            Entry {
                glyph: "🍼",
                png: include_bytes!("../assets/emoji/1f37c.png"),
            },
            Entry {
                glyph: "🍽️",
                png: include_bytes!("../assets/emoji/1f37d.png"),
            },
            Entry {
                glyph: "🍾",
                png: include_bytes!("../assets/emoji/1f37e.png"),
            },
            Entry {
                glyph: "🍿",
                png: include_bytes!("../assets/emoji/1f37f.png"),
            },
            Entry {
                glyph: "🥐",
                png: include_bytes!("../assets/emoji/1f950.png"),
            },
            Entry {
                glyph: "🥑",
                png: include_bytes!("../assets/emoji/1f951.png"),
            },
            Entry {
                glyph: "🥒",
                png: include_bytes!("../assets/emoji/1f952.png"),
            },
            Entry {
                glyph: "🥓",
                png: include_bytes!("../assets/emoji/1f953.png"),
            },
            Entry {
                glyph: "🥔",
                png: include_bytes!("../assets/emoji/1f954.png"),
            },
            Entry {
                glyph: "🥕",
                png: include_bytes!("../assets/emoji/1f955.png"),
            },
            Entry {
                glyph: "🥖",
                png: include_bytes!("../assets/emoji/1f956.png"),
            },
            Entry {
                glyph: "🥗",
                png: include_bytes!("../assets/emoji/1f957.png"),
            },
            Entry {
                glyph: "🥘",
                png: include_bytes!("../assets/emoji/1f958.png"),
            },
            Entry {
                glyph: "🥙",
                png: include_bytes!("../assets/emoji/1f959.png"),
            },
            Entry {
                glyph: "🥚",
                png: include_bytes!("../assets/emoji/1f95a.png"),
            },
            Entry {
                glyph: "🥛",
                png: include_bytes!("../assets/emoji/1f95b.png"),
            },
            Entry {
                glyph: "🥜",
                png: include_bytes!("../assets/emoji/1f95c.png"),
            },
            Entry {
                glyph: "🥝",
                png: include_bytes!("../assets/emoji/1f95d.png"),
            },
            Entry {
                glyph: "🥞",
                png: include_bytes!("../assets/emoji/1f95e.png"),
            },
            Entry {
                glyph: "🥟",
                png: include_bytes!("../assets/emoji/1f95f.png"),
            },
            Entry {
                glyph: "🥠",
                png: include_bytes!("../assets/emoji/1f960.png"),
            },
            Entry {
                glyph: "🥡",
                png: include_bytes!("../assets/emoji/1f961.png"),
            },
            Entry {
                glyph: "🥢",
                png: include_bytes!("../assets/emoji/1f962.png"),
            },
            Entry {
                glyph: "🥣",
                png: include_bytes!("../assets/emoji/1f963.png"),
            },
            Entry {
                glyph: "🥤",
                png: include_bytes!("../assets/emoji/1f964.png"),
            },
            Entry {
                glyph: "🥥",
                png: include_bytes!("../assets/emoji/1f965.png"),
            },
            Entry {
                glyph: "🥦",
                png: include_bytes!("../assets/emoji/1f966.png"),
            },
            Entry {
                glyph: "🥧",
                png: include_bytes!("../assets/emoji/1f967.png"),
            },
            Entry {
                glyph: "🥨",
                png: include_bytes!("../assets/emoji/1f968.png"),
            },
            Entry {
                glyph: "🥩",
                png: include_bytes!("../assets/emoji/1f969.png"),
            },
            Entry {
                glyph: "🥪",
                png: include_bytes!("../assets/emoji/1f96a.png"),
            },
            Entry {
                glyph: "🥫",
                png: include_bytes!("../assets/emoji/1f96b.png"),
            },
            Entry {
                glyph: "🌭",
                png: include_bytes!("../assets/emoji/1f32d.png"),
            },
            Entry {
                glyph: "🌮",
                png: include_bytes!("../assets/emoji/1f32e.png"),
            },
            Entry {
                glyph: "🌯",
                png: include_bytes!("../assets/emoji/1f32f.png"),
            },
            Entry {
                glyph: "🧀",
                png: include_bytes!("../assets/emoji/1f9c0.png"),
            },
            Entry {
                glyph: "🧁",
                png: include_bytes!("../assets/emoji/1f9c1.png"),
            },
            Entry {
                glyph: "🧂",
                png: include_bytes!("../assets/emoji/1f9c2.png"),
            },
            Entry {
                glyph: "☕",
                png: include_bytes!("../assets/emoji/2615.png"),
            },
            Entry {
                glyph: "🥬",
                png: include_bytes!("../assets/emoji/1f96c.png"),
            },
            Entry {
                glyph: "🥭",
                png: include_bytes!("../assets/emoji/1f96d.png"),
            },
            Entry {
                glyph: "🥮",
                png: include_bytes!("../assets/emoji/1f96e.png"),
            },
            Entry {
                glyph: "🥯",
                png: include_bytes!("../assets/emoji/1f96f.png"),
            },
        ],
    ),
    (
        "Активности",
        "⚽",
        &[
            Entry {
                glyph: "🎠",
                png: include_bytes!("../assets/emoji/1f3a0.png"),
            },
            Entry {
                glyph: "🎡",
                png: include_bytes!("../assets/emoji/1f3a1.png"),
            },
            Entry {
                glyph: "🎢",
                png: include_bytes!("../assets/emoji/1f3a2.png"),
            },
            Entry {
                glyph: "🎣",
                png: include_bytes!("../assets/emoji/1f3a3.png"),
            },
            Entry {
                glyph: "🎤",
                png: include_bytes!("../assets/emoji/1f3a4.png"),
            },
            Entry {
                glyph: "🎥",
                png: include_bytes!("../assets/emoji/1f3a5.png"),
            },
            Entry {
                glyph: "🎦",
                png: include_bytes!("../assets/emoji/1f3a6.png"),
            },
            Entry {
                glyph: "🎧",
                png: include_bytes!("../assets/emoji/1f3a7.png"),
            },
            Entry {
                glyph: "🎨",
                png: include_bytes!("../assets/emoji/1f3a8.png"),
            },
            Entry {
                glyph: "🎩",
                png: include_bytes!("../assets/emoji/1f3a9.png"),
            },
            Entry {
                glyph: "🎪",
                png: include_bytes!("../assets/emoji/1f3aa.png"),
            },
            Entry {
                glyph: "🎫",
                png: include_bytes!("../assets/emoji/1f3ab.png"),
            },
            Entry {
                glyph: "🎬",
                png: include_bytes!("../assets/emoji/1f3ac.png"),
            },
            Entry {
                glyph: "🎭",
                png: include_bytes!("../assets/emoji/1f3ad.png"),
            },
            Entry {
                glyph: "🎮",
                png: include_bytes!("../assets/emoji/1f3ae.png"),
            },
            Entry {
                glyph: "🎯",
                png: include_bytes!("../assets/emoji/1f3af.png"),
            },
            Entry {
                glyph: "🎰",
                png: include_bytes!("../assets/emoji/1f3b0.png"),
            },
            Entry {
                glyph: "🎱",
                png: include_bytes!("../assets/emoji/1f3b1.png"),
            },
            Entry {
                glyph: "🎲",
                png: include_bytes!("../assets/emoji/1f3b2.png"),
            },
            Entry {
                glyph: "🎳",
                png: include_bytes!("../assets/emoji/1f3b3.png"),
            },
            Entry {
                glyph: "🎴",
                png: include_bytes!("../assets/emoji/1f3b4.png"),
            },
            Entry {
                glyph: "🎵",
                png: include_bytes!("../assets/emoji/1f3b5.png"),
            },
            Entry {
                glyph: "🎶",
                png: include_bytes!("../assets/emoji/1f3b6.png"),
            },
            Entry {
                glyph: "🎷",
                png: include_bytes!("../assets/emoji/1f3b7.png"),
            },
            Entry {
                glyph: "🎸",
                png: include_bytes!("../assets/emoji/1f3b8.png"),
            },
            Entry {
                glyph: "🎹",
                png: include_bytes!("../assets/emoji/1f3b9.png"),
            },
            Entry {
                glyph: "🎺",
                png: include_bytes!("../assets/emoji/1f3ba.png"),
            },
            Entry {
                glyph: "🎻",
                png: include_bytes!("../assets/emoji/1f3bb.png"),
            },
            Entry {
                glyph: "🎼",
                png: include_bytes!("../assets/emoji/1f3bc.png"),
            },
            Entry {
                glyph: "🎽",
                png: include_bytes!("../assets/emoji/1f3bd.png"),
            },
            Entry {
                glyph: "🎾",
                png: include_bytes!("../assets/emoji/1f3be.png"),
            },
            Entry {
                glyph: "🎿",
                png: include_bytes!("../assets/emoji/1f3bf.png"),
            },
            Entry {
                glyph: "🏀",
                png: include_bytes!("../assets/emoji/1f3c0.png"),
            },
            Entry {
                glyph: "🏁",
                png: include_bytes!("../assets/emoji/1f3c1.png"),
            },
            Entry {
                glyph: "🏂",
                png: include_bytes!("../assets/emoji/1f3c2.png"),
            },
            Entry {
                glyph: "🏃",
                png: include_bytes!("../assets/emoji/1f3c3.png"),
            },
            Entry {
                glyph: "🏄",
                png: include_bytes!("../assets/emoji/1f3c4.png"),
            },
            Entry {
                glyph: "🏅",
                png: include_bytes!("../assets/emoji/1f3c5.png"),
            },
            Entry {
                glyph: "🏆",
                png: include_bytes!("../assets/emoji/1f3c6.png"),
            },
            Entry {
                glyph: "🏇",
                png: include_bytes!("../assets/emoji/1f3c7.png"),
            },
            Entry {
                glyph: "🏈",
                png: include_bytes!("../assets/emoji/1f3c8.png"),
            },
            Entry {
                glyph: "🏉",
                png: include_bytes!("../assets/emoji/1f3c9.png"),
            },
            Entry {
                glyph: "🏊",
                png: include_bytes!("../assets/emoji/1f3ca.png"),
            },
            Entry {
                glyph: "🏏",
                png: include_bytes!("../assets/emoji/1f3cf.png"),
            },
            Entry {
                glyph: "🏐",
                png: include_bytes!("../assets/emoji/1f3d0.png"),
            },
            Entry {
                glyph: "🏑",
                png: include_bytes!("../assets/emoji/1f3d1.png"),
            },
            Entry {
                glyph: "🏒",
                png: include_bytes!("../assets/emoji/1f3d2.png"),
            },
            Entry {
                glyph: "🏓",
                png: include_bytes!("../assets/emoji/1f3d3.png"),
            },
            Entry {
                glyph: "⚽",
                png: include_bytes!("../assets/emoji/26bd.png"),
            },
            Entry {
                glyph: "⚾",
                png: include_bytes!("../assets/emoji/26be.png"),
            },
            Entry {
                glyph: "🥊",
                png: include_bytes!("../assets/emoji/1f94a.png"),
            },
            Entry {
                glyph: "🥋",
                png: include_bytes!("../assets/emoji/1f94b.png"),
            },
            Entry {
                glyph: "🥌",
                png: include_bytes!("../assets/emoji/1f94c.png"),
            },
            Entry {
                glyph: "🥍",
                png: include_bytes!("../assets/emoji/1f94d.png"),
            },
            Entry {
                glyph: "🥎",
                png: include_bytes!("../assets/emoji/1f94e.png"),
            },
            Entry {
                glyph: "🥏",
                png: include_bytes!("../assets/emoji/1f94f.png"),
            },
            Entry {
                glyph: "🧩",
                png: include_bytes!("../assets/emoji/1f9e9.png"),
            },
        ],
    ),
    (
        "Места",
        "🚗",
        &[
            Entry {
                glyph: "🚀",
                png: include_bytes!("../assets/emoji/1f680.png"),
            },
            Entry {
                glyph: "🚁",
                png: include_bytes!("../assets/emoji/1f681.png"),
            },
            Entry {
                glyph: "🚂",
                png: include_bytes!("../assets/emoji/1f682.png"),
            },
            Entry {
                glyph: "🚃",
                png: include_bytes!("../assets/emoji/1f683.png"),
            },
            Entry {
                glyph: "🚄",
                png: include_bytes!("../assets/emoji/1f684.png"),
            },
            Entry {
                glyph: "🚅",
                png: include_bytes!("../assets/emoji/1f685.png"),
            },
            Entry {
                glyph: "🚆",
                png: include_bytes!("../assets/emoji/1f686.png"),
            },
            Entry {
                glyph: "🚇",
                png: include_bytes!("../assets/emoji/1f687.png"),
            },
            Entry {
                glyph: "🚈",
                png: include_bytes!("../assets/emoji/1f688.png"),
            },
            Entry {
                glyph: "🚉",
                png: include_bytes!("../assets/emoji/1f689.png"),
            },
            Entry {
                glyph: "🚊",
                png: include_bytes!("../assets/emoji/1f68a.png"),
            },
            Entry {
                glyph: "🚋",
                png: include_bytes!("../assets/emoji/1f68b.png"),
            },
            Entry {
                glyph: "🚌",
                png: include_bytes!("../assets/emoji/1f68c.png"),
            },
            Entry {
                glyph: "🚍",
                png: include_bytes!("../assets/emoji/1f68d.png"),
            },
            Entry {
                glyph: "🚎",
                png: include_bytes!("../assets/emoji/1f68e.png"),
            },
            Entry {
                glyph: "🚏",
                png: include_bytes!("../assets/emoji/1f68f.png"),
            },
            Entry {
                glyph: "🚐",
                png: include_bytes!("../assets/emoji/1f690.png"),
            },
            Entry {
                glyph: "🚑",
                png: include_bytes!("../assets/emoji/1f691.png"),
            },
            Entry {
                glyph: "🚒",
                png: include_bytes!("../assets/emoji/1f692.png"),
            },
            Entry {
                glyph: "🚓",
                png: include_bytes!("../assets/emoji/1f693.png"),
            },
            Entry {
                glyph: "🚔",
                png: include_bytes!("../assets/emoji/1f694.png"),
            },
            Entry {
                glyph: "🚕",
                png: include_bytes!("../assets/emoji/1f695.png"),
            },
            Entry {
                glyph: "🚖",
                png: include_bytes!("../assets/emoji/1f696.png"),
            },
            Entry {
                glyph: "🚗",
                png: include_bytes!("../assets/emoji/1f697.png"),
            },
            Entry {
                glyph: "🚘",
                png: include_bytes!("../assets/emoji/1f698.png"),
            },
            Entry {
                glyph: "🚙",
                png: include_bytes!("../assets/emoji/1f699.png"),
            },
            Entry {
                glyph: "🚚",
                png: include_bytes!("../assets/emoji/1f69a.png"),
            },
            Entry {
                glyph: "🚛",
                png: include_bytes!("../assets/emoji/1f69b.png"),
            },
            Entry {
                glyph: "🚜",
                png: include_bytes!("../assets/emoji/1f69c.png"),
            },
            Entry {
                glyph: "🚝",
                png: include_bytes!("../assets/emoji/1f69d.png"),
            },
            Entry {
                glyph: "🚞",
                png: include_bytes!("../assets/emoji/1f69e.png"),
            },
            Entry {
                glyph: "🚟",
                png: include_bytes!("../assets/emoji/1f69f.png"),
            },
            Entry {
                glyph: "🚠",
                png: include_bytes!("../assets/emoji/1f6a0.png"),
            },
            Entry {
                glyph: "🚡",
                png: include_bytes!("../assets/emoji/1f6a1.png"),
            },
            Entry {
                glyph: "🚢",
                png: include_bytes!("../assets/emoji/1f6a2.png"),
            },
            Entry {
                glyph: "🚣",
                png: include_bytes!("../assets/emoji/1f6a3.png"),
            },
            Entry {
                glyph: "🚤",
                png: include_bytes!("../assets/emoji/1f6a4.png"),
            },
            Entry {
                glyph: "🏔️",
                png: include_bytes!("../assets/emoji/1f3d4.png"),
            },
            Entry {
                glyph: "🏕️",
                png: include_bytes!("../assets/emoji/1f3d5.png"),
            },
            Entry {
                glyph: "🏖️",
                png: include_bytes!("../assets/emoji/1f3d6.png"),
            },
            Entry {
                glyph: "🏗️",
                png: include_bytes!("../assets/emoji/1f3d7.png"),
            },
            Entry {
                glyph: "🏘️",
                png: include_bytes!("../assets/emoji/1f3d8.png"),
            },
            Entry {
                glyph: "🏙️",
                png: include_bytes!("../assets/emoji/1f3d9.png"),
            },
            Entry {
                glyph: "🏚️",
                png: include_bytes!("../assets/emoji/1f3da.png"),
            },
            Entry {
                glyph: "🏛️",
                png: include_bytes!("../assets/emoji/1f3db.png"),
            },
            Entry {
                glyph: "🏜️",
                png: include_bytes!("../assets/emoji/1f3dc.png"),
            },
            Entry {
                glyph: "🏝️",
                png: include_bytes!("../assets/emoji/1f3dd.png"),
            },
            Entry {
                glyph: "🏞️",
                png: include_bytes!("../assets/emoji/1f3de.png"),
            },
            Entry {
                glyph: "🏟️",
                png: include_bytes!("../assets/emoji/1f3df.png"),
            },
            Entry {
                glyph: "🏠",
                png: include_bytes!("../assets/emoji/1f3e0.png"),
            },
            Entry {
                glyph: "🏡",
                png: include_bytes!("../assets/emoji/1f3e1.png"),
            },
            Entry {
                glyph: "🏢",
                png: include_bytes!("../assets/emoji/1f3e2.png"),
            },
            Entry {
                glyph: "🏣",
                png: include_bytes!("../assets/emoji/1f3e3.png"),
            },
            Entry {
                glyph: "🏤",
                png: include_bytes!("../assets/emoji/1f3e4.png"),
            },
            Entry {
                glyph: "🏥",
                png: include_bytes!("../assets/emoji/1f3e5.png"),
            },
            Entry {
                glyph: "🏦",
                png: include_bytes!("../assets/emoji/1f3e6.png"),
            },
            Entry {
                glyph: "🏧",
                png: include_bytes!("../assets/emoji/1f3e7.png"),
            },
            Entry {
                glyph: "🏨",
                png: include_bytes!("../assets/emoji/1f3e8.png"),
            },
            Entry {
                glyph: "🏩",
                png: include_bytes!("../assets/emoji/1f3e9.png"),
            },
            Entry {
                glyph: "🏪",
                png: include_bytes!("../assets/emoji/1f3ea.png"),
            },
            Entry {
                glyph: "🏫",
                png: include_bytes!("../assets/emoji/1f3eb.png"),
            },
            Entry {
                glyph: "🏬",
                png: include_bytes!("../assets/emoji/1f3ec.png"),
            },
            Entry {
                glyph: "🏭",
                png: include_bytes!("../assets/emoji/1f3ed.png"),
            },
            Entry {
                glyph: "🏮",
                png: include_bytes!("../assets/emoji/1f3ee.png"),
            },
            Entry {
                glyph: "🏯",
                png: include_bytes!("../assets/emoji/1f3ef.png"),
            },
            Entry {
                glyph: "🏰",
                png: include_bytes!("../assets/emoji/1f3f0.png"),
            },
            Entry {
                glyph: "🗺️",
                png: include_bytes!("../assets/emoji/1f5fa.png"),
            },
            Entry {
                glyph: "🗻",
                png: include_bytes!("../assets/emoji/1f5fb.png"),
            },
            Entry {
                glyph: "🗼",
                png: include_bytes!("../assets/emoji/1f5fc.png"),
            },
            Entry {
                glyph: "🗽",
                png: include_bytes!("../assets/emoji/1f5fd.png"),
            },
            Entry {
                glyph: "🗾",
                png: include_bytes!("../assets/emoji/1f5fe.png"),
            },
            Entry {
                glyph: "🗿",
                png: include_bytes!("../assets/emoji/1f5ff.png"),
            },
            Entry {
                glyph: "⛰️",
                png: include_bytes!("../assets/emoji/26f0.png"),
            },
            Entry {
                glyph: "⛱️",
                png: include_bytes!("../assets/emoji/26f1.png"),
            },
            Entry {
                glyph: "⛲",
                png: include_bytes!("../assets/emoji/26f2.png"),
            },
            Entry {
                glyph: "⛳",
                png: include_bytes!("../assets/emoji/26f3.png"),
            },
            Entry {
                glyph: "⛴️",
                png: include_bytes!("../assets/emoji/26f4.png"),
            },
            Entry {
                glyph: "⛵",
                png: include_bytes!("../assets/emoji/26f5.png"),
            },
            Entry {
                glyph: "⛷️",
                png: include_bytes!("../assets/emoji/26f7.png"),
            },
            Entry {
                glyph: "⛸️",
                png: include_bytes!("../assets/emoji/26f8.png"),
            },
            Entry {
                glyph: "⛹️",
                png: include_bytes!("../assets/emoji/26f9.png"),
            },
            Entry {
                glyph: "⛺",
                png: include_bytes!("../assets/emoji/26fa.png"),
            },
            Entry {
                glyph: "🚥",
                png: include_bytes!("../assets/emoji/1f6a5.png"),
            },
            Entry {
                glyph: "🚦",
                png: include_bytes!("../assets/emoji/1f6a6.png"),
            },
            Entry {
                glyph: "🚧",
                png: include_bytes!("../assets/emoji/1f6a7.png"),
            },
            Entry {
                glyph: "🚨",
                png: include_bytes!("../assets/emoji/1f6a8.png"),
            },
            Entry {
                glyph: "🚩",
                png: include_bytes!("../assets/emoji/1f6a9.png"),
            },
            Entry {
                glyph: "🚪",
                png: include_bytes!("../assets/emoji/1f6aa.png"),
            },
            Entry {
                glyph: "🚫",
                png: include_bytes!("../assets/emoji/1f6ab.png"),
            },
            Entry {
                glyph: "🚬",
                png: include_bytes!("../assets/emoji/1f6ac.png"),
            },
            Entry {
                glyph: "🚭",
                png: include_bytes!("../assets/emoji/1f6ad.png"),
            },
            Entry {
                glyph: "🚮",
                png: include_bytes!("../assets/emoji/1f6ae.png"),
            },
            Entry {
                glyph: "🚯",
                png: include_bytes!("../assets/emoji/1f6af.png"),
            },
            Entry {
                glyph: "🚰",
                png: include_bytes!("../assets/emoji/1f6b0.png"),
            },
            Entry {
                glyph: "🚱",
                png: include_bytes!("../assets/emoji/1f6b1.png"),
            },
            Entry {
                glyph: "🚲",
                png: include_bytes!("../assets/emoji/1f6b2.png"),
            },
            Entry {
                glyph: "🚳",
                png: include_bytes!("../assets/emoji/1f6b3.png"),
            },
        ],
    ),
    (
        "Предметы",
        "💡",
        &[
            Entry {
                glyph: "💡",
                png: include_bytes!("../assets/emoji/1f4a1.png"),
            },
            Entry {
                glyph: "💢",
                png: include_bytes!("../assets/emoji/1f4a2.png"),
            },
            Entry {
                glyph: "💣",
                png: include_bytes!("../assets/emoji/1f4a3.png"),
            },
            Entry {
                glyph: "💤",
                png: include_bytes!("../assets/emoji/1f4a4.png"),
            },
            Entry {
                glyph: "💥",
                png: include_bytes!("../assets/emoji/1f4a5.png"),
            },
            Entry {
                glyph: "💦",
                png: include_bytes!("../assets/emoji/1f4a6.png"),
            },
            Entry {
                glyph: "💧",
                png: include_bytes!("../assets/emoji/1f4a7.png"),
            },
            Entry {
                glyph: "💨",
                png: include_bytes!("../assets/emoji/1f4a8.png"),
            },
            Entry {
                glyph: "💩",
                png: include_bytes!("../assets/emoji/1f4a9.png"),
            },
            Entry {
                glyph: "💰",
                png: include_bytes!("../assets/emoji/1f4b0.png"),
            },
            Entry {
                glyph: "💱",
                png: include_bytes!("../assets/emoji/1f4b1.png"),
            },
            Entry {
                glyph: "💲",
                png: include_bytes!("../assets/emoji/1f4b2.png"),
            },
            Entry {
                glyph: "💳",
                png: include_bytes!("../assets/emoji/1f4b3.png"),
            },
            Entry {
                glyph: "💴",
                png: include_bytes!("../assets/emoji/1f4b4.png"),
            },
            Entry {
                glyph: "💵",
                png: include_bytes!("../assets/emoji/1f4b5.png"),
            },
            Entry {
                glyph: "💶",
                png: include_bytes!("../assets/emoji/1f4b6.png"),
            },
            Entry {
                glyph: "💷",
                png: include_bytes!("../assets/emoji/1f4b7.png"),
            },
            Entry {
                glyph: "💸",
                png: include_bytes!("../assets/emoji/1f4b8.png"),
            },
            Entry {
                glyph: "💹",
                png: include_bytes!("../assets/emoji/1f4b9.png"),
            },
            Entry {
                glyph: "💺",
                png: include_bytes!("../assets/emoji/1f4ba.png"),
            },
            Entry {
                glyph: "💻",
                png: include_bytes!("../assets/emoji/1f4bb.png"),
            },
            Entry {
                glyph: "💼",
                png: include_bytes!("../assets/emoji/1f4bc.png"),
            },
            Entry {
                glyph: "💽",
                png: include_bytes!("../assets/emoji/1f4bd.png"),
            },
            Entry {
                glyph: "💾",
                png: include_bytes!("../assets/emoji/1f4be.png"),
            },
            Entry {
                glyph: "💿",
                png: include_bytes!("../assets/emoji/1f4bf.png"),
            },
            Entry {
                glyph: "📀",
                png: include_bytes!("../assets/emoji/1f4c0.png"),
            },
            Entry {
                glyph: "📁",
                png: include_bytes!("../assets/emoji/1f4c1.png"),
            },
            Entry {
                glyph: "📂",
                png: include_bytes!("../assets/emoji/1f4c2.png"),
            },
            Entry {
                glyph: "📃",
                png: include_bytes!("../assets/emoji/1f4c3.png"),
            },
            Entry {
                glyph: "📄",
                png: include_bytes!("../assets/emoji/1f4c4.png"),
            },
            Entry {
                glyph: "📅",
                png: include_bytes!("../assets/emoji/1f4c5.png"),
            },
            Entry {
                glyph: "📆",
                png: include_bytes!("../assets/emoji/1f4c6.png"),
            },
            Entry {
                glyph: "📇",
                png: include_bytes!("../assets/emoji/1f4c7.png"),
            },
            Entry {
                glyph: "📈",
                png: include_bytes!("../assets/emoji/1f4c8.png"),
            },
            Entry {
                glyph: "📉",
                png: include_bytes!("../assets/emoji/1f4c9.png"),
            },
            Entry {
                glyph: "📊",
                png: include_bytes!("../assets/emoji/1f4ca.png"),
            },
            Entry {
                glyph: "📋",
                png: include_bytes!("../assets/emoji/1f4cb.png"),
            },
            Entry {
                glyph: "📌",
                png: include_bytes!("../assets/emoji/1f4cc.png"),
            },
            Entry {
                glyph: "📍",
                png: include_bytes!("../assets/emoji/1f4cd.png"),
            },
            Entry {
                glyph: "📎",
                png: include_bytes!("../assets/emoji/1f4ce.png"),
            },
            Entry {
                glyph: "📏",
                png: include_bytes!("../assets/emoji/1f4cf.png"),
            },
            Entry {
                glyph: "📐",
                png: include_bytes!("../assets/emoji/1f4d0.png"),
            },
            Entry {
                glyph: "📑",
                png: include_bytes!("../assets/emoji/1f4d1.png"),
            },
            Entry {
                glyph: "📒",
                png: include_bytes!("../assets/emoji/1f4d2.png"),
            },
            Entry {
                glyph: "📓",
                png: include_bytes!("../assets/emoji/1f4d3.png"),
            },
            Entry {
                glyph: "📔",
                png: include_bytes!("../assets/emoji/1f4d4.png"),
            },
            Entry {
                glyph: "📕",
                png: include_bytes!("../assets/emoji/1f4d5.png"),
            },
            Entry {
                glyph: "📖",
                png: include_bytes!("../assets/emoji/1f4d6.png"),
            },
            Entry {
                glyph: "📗",
                png: include_bytes!("../assets/emoji/1f4d7.png"),
            },
            Entry {
                glyph: "📘",
                png: include_bytes!("../assets/emoji/1f4d8.png"),
            },
            Entry {
                glyph: "📙",
                png: include_bytes!("../assets/emoji/1f4d9.png"),
            },
            Entry {
                glyph: "📚",
                png: include_bytes!("../assets/emoji/1f4da.png"),
            },
            Entry {
                glyph: "📛",
                png: include_bytes!("../assets/emoji/1f4db.png"),
            },
            Entry {
                glyph: "📜",
                png: include_bytes!("../assets/emoji/1f4dc.png"),
            },
            Entry {
                glyph: "📝",
                png: include_bytes!("../assets/emoji/1f4dd.png"),
            },
            Entry {
                glyph: "📞",
                png: include_bytes!("../assets/emoji/1f4de.png"),
            },
            Entry {
                glyph: "📟",
                png: include_bytes!("../assets/emoji/1f4df.png"),
            },
            Entry {
                glyph: "📠",
                png: include_bytes!("../assets/emoji/1f4e0.png"),
            },
            Entry {
                glyph: "📡",
                png: include_bytes!("../assets/emoji/1f4e1.png"),
            },
            Entry {
                glyph: "📢",
                png: include_bytes!("../assets/emoji/1f4e2.png"),
            },
            Entry {
                glyph: "📣",
                png: include_bytes!("../assets/emoji/1f4e3.png"),
            },
            Entry {
                glyph: "📤",
                png: include_bytes!("../assets/emoji/1f4e4.png"),
            },
            Entry {
                glyph: "📥",
                png: include_bytes!("../assets/emoji/1f4e5.png"),
            },
            Entry {
                glyph: "📦",
                png: include_bytes!("../assets/emoji/1f4e6.png"),
            },
            Entry {
                glyph: "📧",
                png: include_bytes!("../assets/emoji/1f4e7.png"),
            },
            Entry {
                glyph: "📨",
                png: include_bytes!("../assets/emoji/1f4e8.png"),
            },
            Entry {
                glyph: "📩",
                png: include_bytes!("../assets/emoji/1f4e9.png"),
            },
            Entry {
                glyph: "📪",
                png: include_bytes!("../assets/emoji/1f4ea.png"),
            },
            Entry {
                glyph: "📫",
                png: include_bytes!("../assets/emoji/1f4eb.png"),
            },
            Entry {
                glyph: "📬",
                png: include_bytes!("../assets/emoji/1f4ec.png"),
            },
            Entry {
                glyph: "📭",
                png: include_bytes!("../assets/emoji/1f4ed.png"),
            },
            Entry {
                glyph: "📮",
                png: include_bytes!("../assets/emoji/1f4ee.png"),
            },
            Entry {
                glyph: "📯",
                png: include_bytes!("../assets/emoji/1f4ef.png"),
            },
            Entry {
                glyph: "📰",
                png: include_bytes!("../assets/emoji/1f4f0.png"),
            },
            Entry {
                glyph: "📱",
                png: include_bytes!("../assets/emoji/1f4f1.png"),
            },
            Entry {
                glyph: "📲",
                png: include_bytes!("../assets/emoji/1f4f2.png"),
            },
            Entry {
                glyph: "📳",
                png: include_bytes!("../assets/emoji/1f4f3.png"),
            },
            Entry {
                glyph: "📴",
                png: include_bytes!("../assets/emoji/1f4f4.png"),
            },
            Entry {
                glyph: "📵",
                png: include_bytes!("../assets/emoji/1f4f5.png"),
            },
            Entry {
                glyph: "📶",
                png: include_bytes!("../assets/emoji/1f4f6.png"),
            },
            Entry {
                glyph: "📷",
                png: include_bytes!("../assets/emoji/1f4f7.png"),
            },
            Entry {
                glyph: "📸",
                png: include_bytes!("../assets/emoji/1f4f8.png"),
            },
            Entry {
                glyph: "📹",
                png: include_bytes!("../assets/emoji/1f4f9.png"),
            },
            Entry {
                glyph: "📺",
                png: include_bytes!("../assets/emoji/1f4fa.png"),
            },
            Entry {
                glyph: "📻",
                png: include_bytes!("../assets/emoji/1f4fb.png"),
            },
            Entry {
                glyph: "📼",
                png: include_bytes!("../assets/emoji/1f4fc.png"),
            },
            Entry {
                glyph: "📽️",
                png: include_bytes!("../assets/emoji/1f4fd.png"),
            },
            Entry {
                glyph: "📿",
                png: include_bytes!("../assets/emoji/1f4ff.png"),
            },
            Entry {
                glyph: "🔀",
                png: include_bytes!("../assets/emoji/1f500.png"),
            },
            Entry {
                glyph: "🔁",
                png: include_bytes!("../assets/emoji/1f501.png"),
            },
            Entry {
                glyph: "🔂",
                png: include_bytes!("../assets/emoji/1f502.png"),
            },
            Entry {
                glyph: "🔃",
                png: include_bytes!("../assets/emoji/1f503.png"),
            },
            Entry {
                glyph: "🔄",
                png: include_bytes!("../assets/emoji/1f504.png"),
            },
            Entry {
                glyph: "🔅",
                png: include_bytes!("../assets/emoji/1f505.png"),
            },
            Entry {
                glyph: "🔆",
                png: include_bytes!("../assets/emoji/1f506.png"),
            },
            Entry {
                glyph: "🔇",
                png: include_bytes!("../assets/emoji/1f507.png"),
            },
            Entry {
                glyph: "🔈",
                png: include_bytes!("../assets/emoji/1f508.png"),
            },
            Entry {
                glyph: "🔉",
                png: include_bytes!("../assets/emoji/1f509.png"),
            },
            Entry {
                glyph: "🔊",
                png: include_bytes!("../assets/emoji/1f50a.png"),
            },
            Entry {
                glyph: "🔋",
                png: include_bytes!("../assets/emoji/1f50b.png"),
            },
            Entry {
                glyph: "🔌",
                png: include_bytes!("../assets/emoji/1f50c.png"),
            },
            Entry {
                glyph: "🔍",
                png: include_bytes!("../assets/emoji/1f50d.png"),
            },
            Entry {
                glyph: "🔎",
                png: include_bytes!("../assets/emoji/1f50e.png"),
            },
            Entry {
                glyph: "🔏",
                png: include_bytes!("../assets/emoji/1f50f.png"),
            },
            Entry {
                glyph: "🔐",
                png: include_bytes!("../assets/emoji/1f510.png"),
            },
            Entry {
                glyph: "🔑",
                png: include_bytes!("../assets/emoji/1f511.png"),
            },
            Entry {
                glyph: "🔒",
                png: include_bytes!("../assets/emoji/1f512.png"),
            },
            Entry {
                glyph: "🔓",
                png: include_bytes!("../assets/emoji/1f513.png"),
            },
            Entry {
                glyph: "🔔",
                png: include_bytes!("../assets/emoji/1f514.png"),
            },
            Entry {
                glyph: "🔕",
                png: include_bytes!("../assets/emoji/1f515.png"),
            },
            Entry {
                glyph: "🔖",
                png: include_bytes!("../assets/emoji/1f516.png"),
            },
            Entry {
                glyph: "🔗",
                png: include_bytes!("../assets/emoji/1f517.png"),
            },
            Entry {
                glyph: "🔘",
                png: include_bytes!("../assets/emoji/1f518.png"),
            },
            Entry {
                glyph: "🔙",
                png: include_bytes!("../assets/emoji/1f519.png"),
            },
            Entry {
                glyph: "🔚",
                png: include_bytes!("../assets/emoji/1f51a.png"),
            },
            Entry {
                glyph: "🔛",
                png: include_bytes!("../assets/emoji/1f51b.png"),
            },
            Entry {
                glyph: "🔜",
                png: include_bytes!("../assets/emoji/1f51c.png"),
            },
            Entry {
                glyph: "🔝",
                png: include_bytes!("../assets/emoji/1f51d.png"),
            },
            Entry {
                glyph: "🔞",
                png: include_bytes!("../assets/emoji/1f51e.png"),
            },
            Entry {
                glyph: "🔟",
                png: include_bytes!("../assets/emoji/1f51f.png"),
            },
            Entry {
                glyph: "🔠",
                png: include_bytes!("../assets/emoji/1f520.png"),
            },
            Entry {
                glyph: "🔡",
                png: include_bytes!("../assets/emoji/1f521.png"),
            },
            Entry {
                glyph: "🔢",
                png: include_bytes!("../assets/emoji/1f522.png"),
            },
            Entry {
                glyph: "🔣",
                png: include_bytes!("../assets/emoji/1f523.png"),
            },
            Entry {
                glyph: "🔤",
                png: include_bytes!("../assets/emoji/1f524.png"),
            },
            Entry {
                glyph: "🔥",
                png: include_bytes!("../assets/emoji/1f525.png"),
            },
            Entry {
                glyph: "🔦",
                png: include_bytes!("../assets/emoji/1f526.png"),
            },
            Entry {
                glyph: "🔧",
                png: include_bytes!("../assets/emoji/1f527.png"),
            },
            Entry {
                glyph: "🔨",
                png: include_bytes!("../assets/emoji/1f528.png"),
            },
            Entry {
                glyph: "🔩",
                png: include_bytes!("../assets/emoji/1f529.png"),
            },
            Entry {
                glyph: "🔪",
                png: include_bytes!("../assets/emoji/1f52a.png"),
            },
            Entry {
                glyph: "🔫",
                png: include_bytes!("../assets/emoji/1f52b.png"),
            },
            Entry {
                glyph: "🔬",
                png: include_bytes!("../assets/emoji/1f52c.png"),
            },
            Entry {
                glyph: "🔭",
                png: include_bytes!("../assets/emoji/1f52d.png"),
            },
            Entry {
                glyph: "🔮",
                png: include_bytes!("../assets/emoji/1f52e.png"),
            },
            Entry {
                glyph: "🔯",
                png: include_bytes!("../assets/emoji/1f52f.png"),
            },
            Entry {
                glyph: "🔰",
                png: include_bytes!("../assets/emoji/1f530.png"),
            },
            Entry {
                glyph: "🔱",
                png: include_bytes!("../assets/emoji/1f531.png"),
            },
            Entry {
                glyph: "🔲",
                png: include_bytes!("../assets/emoji/1f532.png"),
            },
            Entry {
                glyph: "🔳",
                png: include_bytes!("../assets/emoji/1f533.png"),
            },
            Entry {
                glyph: "🔴",
                png: include_bytes!("../assets/emoji/1f534.png"),
            },
            Entry {
                glyph: "🔵",
                png: include_bytes!("../assets/emoji/1f535.png"),
            },
            Entry {
                glyph: "🔶",
                png: include_bytes!("../assets/emoji/1f536.png"),
            },
            Entry {
                glyph: "🔷",
                png: include_bytes!("../assets/emoji/1f537.png"),
            },
            Entry {
                glyph: "🔸",
                png: include_bytes!("../assets/emoji/1f538.png"),
            },
            Entry {
                glyph: "🔹",
                png: include_bytes!("../assets/emoji/1f539.png"),
            },
            Entry {
                glyph: "🔺",
                png: include_bytes!("../assets/emoji/1f53a.png"),
            },
            Entry {
                glyph: "🔻",
                png: include_bytes!("../assets/emoji/1f53b.png"),
            },
            Entry {
                glyph: "🔼",
                png: include_bytes!("../assets/emoji/1f53c.png"),
            },
            Entry {
                glyph: "🔽",
                png: include_bytes!("../assets/emoji/1f53d.png"),
            },
            Entry {
                glyph: "⌚",
                png: include_bytes!("../assets/emoji/231a.png"),
            },
            Entry {
                glyph: "⌛",
                png: include_bytes!("../assets/emoji/231b.png"),
            },
            Entry {
                glyph: "⌨️",
                png: include_bytes!("../assets/emoji/2328.png"),
            },
            Entry {
                glyph: "⏰",
                png: include_bytes!("../assets/emoji/23f0.png"),
            },
            Entry {
                glyph: "⏱️",
                png: include_bytes!("../assets/emoji/23f1.png"),
            },
            Entry {
                glyph: "⏲️",
                png: include_bytes!("../assets/emoji/23f2.png"),
            },
            Entry {
                glyph: "⏳",
                png: include_bytes!("../assets/emoji/23f3.png"),
            },
            Entry {
                glyph: "🖥️",
                png: include_bytes!("../assets/emoji/1f5a5.png"),
            },
            Entry {
                glyph: "🗂️",
                png: include_bytes!("../assets/emoji/1f5c2.png"),
            },
            Entry {
                glyph: "🗃️",
                png: include_bytes!("../assets/emoji/1f5c3.png"),
            },
            Entry {
                glyph: "🗄️",
                png: include_bytes!("../assets/emoji/1f5c4.png"),
            },
        ],
    ),
    (
        "Символы",
        "❤️",
        &[
            Entry {
                glyph: "❤️",
                png: include_bytes!("../assets/emoji/2764.png"),
            },
            Entry {
                glyph: "💓",
                png: include_bytes!("../assets/emoji/1f493.png"),
            },
            Entry {
                glyph: "💔",
                png: include_bytes!("../assets/emoji/1f494.png"),
            },
            Entry {
                glyph: "💕",
                png: include_bytes!("../assets/emoji/1f495.png"),
            },
            Entry {
                glyph: "💖",
                png: include_bytes!("../assets/emoji/1f496.png"),
            },
            Entry {
                glyph: "💗",
                png: include_bytes!("../assets/emoji/1f497.png"),
            },
            Entry {
                glyph: "💘",
                png: include_bytes!("../assets/emoji/1f498.png"),
            },
            Entry {
                glyph: "💙",
                png: include_bytes!("../assets/emoji/1f499.png"),
            },
            Entry {
                glyph: "💚",
                png: include_bytes!("../assets/emoji/1f49a.png"),
            },
            Entry {
                glyph: "💛",
                png: include_bytes!("../assets/emoji/1f49b.png"),
            },
            Entry {
                glyph: "💜",
                png: include_bytes!("../assets/emoji/1f49c.png"),
            },
            Entry {
                glyph: "💝",
                png: include_bytes!("../assets/emoji/1f49d.png"),
            },
            Entry {
                glyph: "💞",
                png: include_bytes!("../assets/emoji/1f49e.png"),
            },
            Entry {
                glyph: "💟",
                png: include_bytes!("../assets/emoji/1f49f.png"),
            },
            Entry {
                glyph: "🧡",
                png: include_bytes!("../assets/emoji/1f9e1.png"),
            },
            Entry {
                glyph: "✅",
                png: include_bytes!("../assets/emoji/2705.png"),
            },
            Entry {
                glyph: "❌",
                png: include_bytes!("../assets/emoji/274c.png"),
            },
            Entry {
                glyph: "❓",
                png: include_bytes!("../assets/emoji/2753.png"),
            },
            Entry {
                glyph: "❔",
                png: include_bytes!("../assets/emoji/2754.png"),
            },
            Entry {
                glyph: "❕",
                png: include_bytes!("../assets/emoji/2755.png"),
            },
            Entry {
                glyph: "❗",
                png: include_bytes!("../assets/emoji/2757.png"),
            },
            Entry {
                glyph: "💯",
                png: include_bytes!("../assets/emoji/1f4af.png"),
            },
            Entry {
                glyph: "⭐",
                png: include_bytes!("../assets/emoji/2b50.png"),
            },
            Entry {
                glyph: "🎉",
                png: include_bytes!("../assets/emoji/1f389.png"),
            },
            Entry {
                glyph: "🎊",
                png: include_bytes!("../assets/emoji/1f38a.png"),
            },
            Entry {
                glyph: "✨",
                png: include_bytes!("../assets/emoji/2728.png"),
            },
            Entry {
                glyph: "⬆️",
                png: include_bytes!("../assets/emoji/2b06.png"),
            },
            Entry {
                glyph: "⬇️",
                png: include_bytes!("../assets/emoji/2b07.png"),
            },
            Entry {
                glyph: "➡️",
                png: include_bytes!("../assets/emoji/27a1.png"),
            },
            Entry {
                glyph: "↔️",
                png: include_bytes!("../assets/emoji/2194.png"),
            },
            Entry {
                glyph: "↕️",
                png: include_bytes!("../assets/emoji/2195.png"),
            },
            Entry {
                glyph: "↖️",
                png: include_bytes!("../assets/emoji/2196.png"),
            },
            Entry {
                glyph: "↗️",
                png: include_bytes!("../assets/emoji/2197.png"),
            },
            Entry {
                glyph: "↘️",
                png: include_bytes!("../assets/emoji/2198.png"),
            },
            Entry {
                glyph: "↙️",
                png: include_bytes!("../assets/emoji/2199.png"),
            },
        ],
    ),
    (
        "Флаги",
        "🇦🇨",
        &[
            Entry {
                glyph: "🇦🇨",
                png: include_bytes!("../assets/emoji/1f1e6-1f1e8.png"),
            },
            Entry {
                glyph: "🇦🇩",
                png: include_bytes!("../assets/emoji/1f1e6-1f1e9.png"),
            },
            Entry {
                glyph: "🇦🇪",
                png: include_bytes!("../assets/emoji/1f1e6-1f1ea.png"),
            },
            Entry {
                glyph: "🇦🇫",
                png: include_bytes!("../assets/emoji/1f1e6-1f1eb.png"),
            },
            Entry {
                glyph: "🇦🇬",
                png: include_bytes!("../assets/emoji/1f1e6-1f1ec.png"),
            },
            Entry {
                glyph: "🇦🇮",
                png: include_bytes!("../assets/emoji/1f1e6-1f1ee.png"),
            },
            Entry {
                glyph: "🇦🇱",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f1.png"),
            },
            Entry {
                glyph: "🇦🇲",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f2.png"),
            },
            Entry {
                glyph: "🇦🇴",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f4.png"),
            },
            Entry {
                glyph: "🇦🇶",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f6.png"),
            },
            Entry {
                glyph: "🇦🇷",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f7.png"),
            },
            Entry {
                glyph: "🇦🇸",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f8.png"),
            },
            Entry {
                glyph: "🇦🇹",
                png: include_bytes!("../assets/emoji/1f1e6-1f1f9.png"),
            },
            Entry {
                glyph: "🇦🇺",
                png: include_bytes!("../assets/emoji/1f1e6-1f1fa.png"),
            },
            Entry {
                glyph: "🇦🇼",
                png: include_bytes!("../assets/emoji/1f1e6-1f1fc.png"),
            },
            Entry {
                glyph: "🇦🇽",
                png: include_bytes!("../assets/emoji/1f1e6-1f1fd.png"),
            },
            Entry {
                glyph: "🇦🇿",
                png: include_bytes!("../assets/emoji/1f1e6-1f1ff.png"),
            },
            Entry {
                glyph: "🇧🇦",
                png: include_bytes!("../assets/emoji/1f1e7-1f1e6.png"),
            },
            Entry {
                glyph: "🇧🇧",
                png: include_bytes!("../assets/emoji/1f1e7-1f1e7.png"),
            },
            Entry {
                glyph: "🇧🇩",
                png: include_bytes!("../assets/emoji/1f1e7-1f1e9.png"),
            },
            Entry {
                glyph: "🇧🇪",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ea.png"),
            },
            Entry {
                glyph: "🇧🇫",
                png: include_bytes!("../assets/emoji/1f1e7-1f1eb.png"),
            },
            Entry {
                glyph: "🇧🇬",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ec.png"),
            },
            Entry {
                glyph: "🇧🇭",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ed.png"),
            },
            Entry {
                glyph: "🇧🇮",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ee.png"),
            },
            Entry {
                glyph: "🇧🇯",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ef.png"),
            },
            Entry {
                glyph: "🇧🇱",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f1.png"),
            },
            Entry {
                glyph: "🇧🇲",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f2.png"),
            },
            Entry {
                glyph: "🇧🇳",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f3.png"),
            },
            Entry {
                glyph: "🇧🇴",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f4.png"),
            },
            Entry {
                glyph: "🇧🇶",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f6.png"),
            },
            Entry {
                glyph: "🇧🇷",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f7.png"),
            },
            Entry {
                glyph: "🇧🇸",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f8.png"),
            },
            Entry {
                glyph: "🇧🇹",
                png: include_bytes!("../assets/emoji/1f1e7-1f1f9.png"),
            },
            Entry {
                glyph: "🇧🇻",
                png: include_bytes!("../assets/emoji/1f1e7-1f1fb.png"),
            },
            Entry {
                glyph: "🇧🇼",
                png: include_bytes!("../assets/emoji/1f1e7-1f1fc.png"),
            },
            Entry {
                glyph: "🇧🇾",
                png: include_bytes!("../assets/emoji/1f1e7-1f1fe.png"),
            },
            Entry {
                glyph: "🇧🇿",
                png: include_bytes!("../assets/emoji/1f1e7-1f1ff.png"),
            },
            Entry {
                glyph: "🇨🇦",
                png: include_bytes!("../assets/emoji/1f1e8-1f1e6.png"),
            },
            Entry {
                glyph: "🇨🇨",
                png: include_bytes!("../assets/emoji/1f1e8-1f1e8.png"),
            },
            Entry {
                glyph: "🇨🇩",
                png: include_bytes!("../assets/emoji/1f1e8-1f1e9.png"),
            },
            Entry {
                glyph: "🇨🇫",
                png: include_bytes!("../assets/emoji/1f1e8-1f1eb.png"),
            },
            Entry {
                glyph: "🇨🇬",
                png: include_bytes!("../assets/emoji/1f1e8-1f1ec.png"),
            },
            Entry {
                glyph: "🇨🇭",
                png: include_bytes!("../assets/emoji/1f1e8-1f1ed.png"),
            },
            Entry {
                glyph: "🇨🇮",
                png: include_bytes!("../assets/emoji/1f1e8-1f1ee.png"),
            },
            Entry {
                glyph: "🇨🇰",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f0.png"),
            },
            Entry {
                glyph: "🇨🇱",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f1.png"),
            },
            Entry {
                glyph: "🇨🇲",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f2.png"),
            },
            Entry {
                glyph: "🇨🇳",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f3.png"),
            },
            Entry {
                glyph: "🇨🇴",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f4.png"),
            },
            Entry {
                glyph: "🇨🇵",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f5.png"),
            },
            Entry {
                glyph: "🇨🇶",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f6.png"),
            },
            Entry {
                glyph: "🇨🇷",
                png: include_bytes!("../assets/emoji/1f1e8-1f1f7.png"),
            },
            Entry {
                glyph: "🇨🇺",
                png: include_bytes!("../assets/emoji/1f1e8-1f1fa.png"),
            },
            Entry {
                glyph: "🇨🇻",
                png: include_bytes!("../assets/emoji/1f1e8-1f1fb.png"),
            },
            Entry {
                glyph: "🇨🇼",
                png: include_bytes!("../assets/emoji/1f1e8-1f1fc.png"),
            },
            Entry {
                glyph: "🇨🇽",
                png: include_bytes!("../assets/emoji/1f1e8-1f1fd.png"),
            },
            Entry {
                glyph: "🇨🇾",
                png: include_bytes!("../assets/emoji/1f1e8-1f1fe.png"),
            },
            Entry {
                glyph: "🇨🇿",
                png: include_bytes!("../assets/emoji/1f1e8-1f1ff.png"),
            },
            Entry {
                glyph: "🇩🇪",
                png: include_bytes!("../assets/emoji/1f1e9-1f1ea.png"),
            },
            Entry {
                glyph: "🇩🇬",
                png: include_bytes!("../assets/emoji/1f1e9-1f1ec.png"),
            },
            Entry {
                glyph: "🇩🇯",
                png: include_bytes!("../assets/emoji/1f1e9-1f1ef.png"),
            },
            Entry {
                glyph: "🇩🇰",
                png: include_bytes!("../assets/emoji/1f1e9-1f1f0.png"),
            },
            Entry {
                glyph: "🇩🇲",
                png: include_bytes!("../assets/emoji/1f1e9-1f1f2.png"),
            },
            Entry {
                glyph: "🇩🇴",
                png: include_bytes!("../assets/emoji/1f1e9-1f1f4.png"),
            },
            Entry {
                glyph: "🇩🇿",
                png: include_bytes!("../assets/emoji/1f1e9-1f1ff.png"),
            },
            Entry {
                glyph: "🇪🇦",
                png: include_bytes!("../assets/emoji/1f1ea-1f1e6.png"),
            },
            Entry {
                glyph: "🇪🇨",
                png: include_bytes!("../assets/emoji/1f1ea-1f1e8.png"),
            },
            Entry {
                glyph: "🇪🇪",
                png: include_bytes!("../assets/emoji/1f1ea-1f1ea.png"),
            },
            Entry {
                glyph: "🇪🇬",
                png: include_bytes!("../assets/emoji/1f1ea-1f1ec.png"),
            },
            Entry {
                glyph: "🇪🇭",
                png: include_bytes!("../assets/emoji/1f1ea-1f1ed.png"),
            },
            Entry {
                glyph: "🇪🇷",
                png: include_bytes!("../assets/emoji/1f1ea-1f1f7.png"),
            },
            Entry {
                glyph: "🇪🇸",
                png: include_bytes!("../assets/emoji/1f1ea-1f1f8.png"),
            },
            Entry {
                glyph: "🇪🇹",
                png: include_bytes!("../assets/emoji/1f1ea-1f1f9.png"),
            },
            Entry {
                glyph: "🇪🇺",
                png: include_bytes!("../assets/emoji/1f1ea-1f1fa.png"),
            },
            Entry {
                glyph: "🇫🇮",
                png: include_bytes!("../assets/emoji/1f1eb-1f1ee.png"),
            },
            Entry {
                glyph: "🇫🇯",
                png: include_bytes!("../assets/emoji/1f1eb-1f1ef.png"),
            },
            Entry {
                glyph: "🇫🇰",
                png: include_bytes!("../assets/emoji/1f1eb-1f1f0.png"),
            },
            Entry {
                glyph: "🇫🇲",
                png: include_bytes!("../assets/emoji/1f1eb-1f1f2.png"),
            },
            Entry {
                glyph: "🇫🇴",
                png: include_bytes!("../assets/emoji/1f1eb-1f1f4.png"),
            },
            Entry {
                glyph: "🇫🇷",
                png: include_bytes!("../assets/emoji/1f1eb-1f1f7.png"),
            },
            Entry {
                glyph: "🇬🇦",
                png: include_bytes!("../assets/emoji/1f1ec-1f1e6.png"),
            },
            Entry {
                glyph: "🇬🇧",
                png: include_bytes!("../assets/emoji/1f1ec-1f1e7.png"),
            },
            Entry {
                glyph: "🇬🇩",
                png: include_bytes!("../assets/emoji/1f1ec-1f1e9.png"),
            },
            Entry {
                glyph: "🇬🇪",
                png: include_bytes!("../assets/emoji/1f1ec-1f1ea.png"),
            },
            Entry {
                glyph: "🇬🇫",
                png: include_bytes!("../assets/emoji/1f1ec-1f1eb.png"),
            },
            Entry {
                glyph: "🇬🇬",
                png: include_bytes!("../assets/emoji/1f1ec-1f1ec.png"),
            },
            Entry {
                glyph: "🇬🇭",
                png: include_bytes!("../assets/emoji/1f1ec-1f1ed.png"),
            },
            Entry {
                glyph: "🇬🇮",
                png: include_bytes!("../assets/emoji/1f1ec-1f1ee.png"),
            },
            Entry {
                glyph: "🇬🇱",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f1.png"),
            },
            Entry {
                glyph: "🇬🇲",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f2.png"),
            },
            Entry {
                glyph: "🇬🇳",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f3.png"),
            },
            Entry {
                glyph: "🇬🇵",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f5.png"),
            },
            Entry {
                glyph: "🇬🇶",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f6.png"),
            },
            Entry {
                glyph: "🇬🇷",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f7.png"),
            },
            Entry {
                glyph: "🇬🇸",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f8.png"),
            },
            Entry {
                glyph: "🇬🇹",
                png: include_bytes!("../assets/emoji/1f1ec-1f1f9.png"),
            },
            Entry {
                glyph: "🇬🇺",
                png: include_bytes!("../assets/emoji/1f1ec-1f1fa.png"),
            },
            Entry {
                glyph: "🇬🇼",
                png: include_bytes!("../assets/emoji/1f1ec-1f1fc.png"),
            },
            Entry {
                glyph: "🇬🇾",
                png: include_bytes!("../assets/emoji/1f1ec-1f1fe.png"),
            },
            Entry {
                glyph: "🇭🇰",
                png: include_bytes!("../assets/emoji/1f1ed-1f1f0.png"),
            },
            Entry {
                glyph: "🇭🇲",
                png: include_bytes!("../assets/emoji/1f1ed-1f1f2.png"),
            },
            Entry {
                glyph: "🇭🇳",
                png: include_bytes!("../assets/emoji/1f1ed-1f1f3.png"),
            },
            Entry {
                glyph: "🇭🇷",
                png: include_bytes!("../assets/emoji/1f1ed-1f1f7.png"),
            },
            Entry {
                glyph: "🇭🇹",
                png: include_bytes!("../assets/emoji/1f1ed-1f1f9.png"),
            },
            Entry {
                glyph: "🇭🇺",
                png: include_bytes!("../assets/emoji/1f1ed-1f1fa.png"),
            },
            Entry {
                glyph: "🇮🇨",
                png: include_bytes!("../assets/emoji/1f1ee-1f1e8.png"),
            },
            Entry {
                glyph: "🇮🇩",
                png: include_bytes!("../assets/emoji/1f1ee-1f1e9.png"),
            },
            Entry {
                glyph: "🇮🇪",
                png: include_bytes!("../assets/emoji/1f1ee-1f1ea.png"),
            },
            Entry {
                glyph: "🇮🇱",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f1.png"),
            },
            Entry {
                glyph: "🇮🇲",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f2.png"),
            },
            Entry {
                glyph: "🇮🇳",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f3.png"),
            },
            Entry {
                glyph: "🇮🇴",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f4.png"),
            },
            Entry {
                glyph: "🇮🇶",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f6.png"),
            },
            Entry {
                glyph: "🇮🇷",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f7.png"),
            },
            Entry {
                glyph: "🇮🇸",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f8.png"),
            },
            Entry {
                glyph: "🇮🇹",
                png: include_bytes!("../assets/emoji/1f1ee-1f1f9.png"),
            },
            Entry {
                glyph: "🇯🇪",
                png: include_bytes!("../assets/emoji/1f1ef-1f1ea.png"),
            },
            Entry {
                glyph: "🇯🇲",
                png: include_bytes!("../assets/emoji/1f1ef-1f1f2.png"),
            },
            Entry {
                glyph: "🇯🇴",
                png: include_bytes!("../assets/emoji/1f1ef-1f1f4.png"),
            },
            Entry {
                glyph: "🇯🇵",
                png: include_bytes!("../assets/emoji/1f1ef-1f1f5.png"),
            },
            Entry {
                glyph: "🇰🇪",
                png: include_bytes!("../assets/emoji/1f1f0-1f1ea.png"),
            },
            Entry {
                glyph: "🇰🇬",
                png: include_bytes!("../assets/emoji/1f1f0-1f1ec.png"),
            },
            Entry {
                glyph: "🇰🇭",
                png: include_bytes!("../assets/emoji/1f1f0-1f1ed.png"),
            },
            Entry {
                glyph: "🇰🇮",
                png: include_bytes!("../assets/emoji/1f1f0-1f1ee.png"),
            },
            Entry {
                glyph: "🇰🇲",
                png: include_bytes!("../assets/emoji/1f1f0-1f1f2.png"),
            },
            Entry {
                glyph: "🇰🇳",
                png: include_bytes!("../assets/emoji/1f1f0-1f1f3.png"),
            },
            Entry {
                glyph: "🇰🇵",
                png: include_bytes!("../assets/emoji/1f1f0-1f1f5.png"),
            },
            Entry {
                glyph: "🇰🇷",
                png: include_bytes!("../assets/emoji/1f1f0-1f1f7.png"),
            },
            Entry {
                glyph: "🇰🇼",
                png: include_bytes!("../assets/emoji/1f1f0-1f1fc.png"),
            },
            Entry {
                glyph: "🇰🇾",
                png: include_bytes!("../assets/emoji/1f1f0-1f1fe.png"),
            },
            Entry {
                glyph: "🇰🇿",
                png: include_bytes!("../assets/emoji/1f1f0-1f1ff.png"),
            },
            Entry {
                glyph: "🇱🇦",
                png: include_bytes!("../assets/emoji/1f1f1-1f1e6.png"),
            },
            Entry {
                glyph: "🇱🇧",
                png: include_bytes!("../assets/emoji/1f1f1-1f1e7.png"),
            },
            Entry {
                glyph: "🇱🇨",
                png: include_bytes!("../assets/emoji/1f1f1-1f1e8.png"),
            },
            Entry {
                glyph: "🇱🇮",
                png: include_bytes!("../assets/emoji/1f1f1-1f1ee.png"),
            },
            Entry {
                glyph: "🇱🇰",
                png: include_bytes!("../assets/emoji/1f1f1-1f1f0.png"),
            },
            Entry {
                glyph: "🇱🇷",
                png: include_bytes!("../assets/emoji/1f1f1-1f1f7.png"),
            },
            Entry {
                glyph: "🇱🇸",
                png: include_bytes!("../assets/emoji/1f1f1-1f1f8.png"),
            },
            Entry {
                glyph: "🇱🇹",
                png: include_bytes!("../assets/emoji/1f1f1-1f1f9.png"),
            },
            Entry {
                glyph: "🇱🇺",
                png: include_bytes!("../assets/emoji/1f1f1-1f1fa.png"),
            },
            Entry {
                glyph: "🇱🇻",
                png: include_bytes!("../assets/emoji/1f1f1-1f1fb.png"),
            },
            Entry {
                glyph: "🇱🇾",
                png: include_bytes!("../assets/emoji/1f1f1-1f1fe.png"),
            },
            Entry {
                glyph: "🇲🇦",
                png: include_bytes!("../assets/emoji/1f1f2-1f1e6.png"),
            },
            Entry {
                glyph: "🇲🇨",
                png: include_bytes!("../assets/emoji/1f1f2-1f1e8.png"),
            },
            Entry {
                glyph: "🇲🇩",
                png: include_bytes!("../assets/emoji/1f1f2-1f1e9.png"),
            },
            Entry {
                glyph: "🇲🇪",
                png: include_bytes!("../assets/emoji/1f1f2-1f1ea.png"),
            },
            Entry {
                glyph: "🇲🇫",
                png: include_bytes!("../assets/emoji/1f1f2-1f1eb.png"),
            },
            Entry {
                glyph: "🇲🇬",
                png: include_bytes!("../assets/emoji/1f1f2-1f1ec.png"),
            },
            Entry {
                glyph: "🇲🇭",
                png: include_bytes!("../assets/emoji/1f1f2-1f1ed.png"),
            },
            Entry {
                glyph: "🇲🇰",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f0.png"),
            },
            Entry {
                glyph: "🇲🇱",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f1.png"),
            },
            Entry {
                glyph: "🇲🇲",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f2.png"),
            },
            Entry {
                glyph: "🇲🇳",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f3.png"),
            },
            Entry {
                glyph: "🇲🇴",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f4.png"),
            },
            Entry {
                glyph: "🇲🇵",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f5.png"),
            },
            Entry {
                glyph: "🇲🇶",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f6.png"),
            },
            Entry {
                glyph: "🇲🇷",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f7.png"),
            },
            Entry {
                glyph: "🇲🇸",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f8.png"),
            },
            Entry {
                glyph: "🇲🇹",
                png: include_bytes!("../assets/emoji/1f1f2-1f1f9.png"),
            },
            Entry {
                glyph: "🇲🇺",
                png: include_bytes!("../assets/emoji/1f1f2-1f1fa.png"),
            },
            Entry {
                glyph: "🇲🇻",
                png: include_bytes!("../assets/emoji/1f1f2-1f1fb.png"),
            },
            Entry {
                glyph: "🇲🇼",
                png: include_bytes!("../assets/emoji/1f1f2-1f1fc.png"),
            },
            Entry {
                glyph: "🇲🇽",
                png: include_bytes!("../assets/emoji/1f1f2-1f1fd.png"),
            },
            Entry {
                glyph: "🇲🇾",
                png: include_bytes!("../assets/emoji/1f1f2-1f1fe.png"),
            },
            Entry {
                glyph: "🇲🇿",
                png: include_bytes!("../assets/emoji/1f1f2-1f1ff.png"),
            },
            Entry {
                glyph: "🇳🇦",
                png: include_bytes!("../assets/emoji/1f1f3-1f1e6.png"),
            },
            Entry {
                glyph: "🇳🇨",
                png: include_bytes!("../assets/emoji/1f1f3-1f1e8.png"),
            },
            Entry {
                glyph: "🇳🇪",
                png: include_bytes!("../assets/emoji/1f1f3-1f1ea.png"),
            },
            Entry {
                glyph: "🇳🇫",
                png: include_bytes!("../assets/emoji/1f1f3-1f1eb.png"),
            },
            Entry {
                glyph: "🇳🇬",
                png: include_bytes!("../assets/emoji/1f1f3-1f1ec.png"),
            },
            Entry {
                glyph: "🇳🇮",
                png: include_bytes!("../assets/emoji/1f1f3-1f1ee.png"),
            },
            Entry {
                glyph: "🇳🇱",
                png: include_bytes!("../assets/emoji/1f1f3-1f1f1.png"),
            },
            Entry {
                glyph: "🇳🇴",
                png: include_bytes!("../assets/emoji/1f1f3-1f1f4.png"),
            },
            Entry {
                glyph: "🇳🇵",
                png: include_bytes!("../assets/emoji/1f1f3-1f1f5.png"),
            },
            Entry {
                glyph: "🇳🇷",
                png: include_bytes!("../assets/emoji/1f1f3-1f1f7.png"),
            },
            Entry {
                glyph: "🇳🇺",
                png: include_bytes!("../assets/emoji/1f1f3-1f1fa.png"),
            },
            Entry {
                glyph: "🇳🇿",
                png: include_bytes!("../assets/emoji/1f1f3-1f1ff.png"),
            },
            Entry {
                glyph: "🇴🇲",
                png: include_bytes!("../assets/emoji/1f1f4-1f1f2.png"),
            },
            Entry {
                glyph: "🇵🇦",
                png: include_bytes!("../assets/emoji/1f1f5-1f1e6.png"),
            },
            Entry {
                glyph: "🇵🇪",
                png: include_bytes!("../assets/emoji/1f1f5-1f1ea.png"),
            },
            Entry {
                glyph: "🇵🇫",
                png: include_bytes!("../assets/emoji/1f1f5-1f1eb.png"),
            },
            Entry {
                glyph: "🇵🇬",
                png: include_bytes!("../assets/emoji/1f1f5-1f1ec.png"),
            },
            Entry {
                glyph: "🇵🇭",
                png: include_bytes!("../assets/emoji/1f1f5-1f1ed.png"),
            },
            Entry {
                glyph: "🇵🇰",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f0.png"),
            },
            Entry {
                glyph: "🇵🇱",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f1.png"),
            },
            Entry {
                glyph: "🇵🇲",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f2.png"),
            },
            Entry {
                glyph: "🇵🇳",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f3.png"),
            },
            Entry {
                glyph: "🇵🇷",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f7.png"),
            },
            Entry {
                glyph: "🇵🇸",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f8.png"),
            },
            Entry {
                glyph: "🇵🇹",
                png: include_bytes!("../assets/emoji/1f1f5-1f1f9.png"),
            },
            Entry {
                glyph: "🇵🇼",
                png: include_bytes!("../assets/emoji/1f1f5-1f1fc.png"),
            },
            Entry {
                glyph: "🇵🇾",
                png: include_bytes!("../assets/emoji/1f1f5-1f1fe.png"),
            },
            Entry {
                glyph: "🇶🇦",
                png: include_bytes!("../assets/emoji/1f1f6-1f1e6.png"),
            },
            Entry {
                glyph: "🇷🇪",
                png: include_bytes!("../assets/emoji/1f1f7-1f1ea.png"),
            },
            Entry {
                glyph: "🇷🇴",
                png: include_bytes!("../assets/emoji/1f1f7-1f1f4.png"),
            },
            Entry {
                glyph: "🇷🇸",
                png: include_bytes!("../assets/emoji/1f1f7-1f1f8.png"),
            },
            Entry {
                glyph: "🇷🇺",
                png: include_bytes!("../assets/emoji/1f1f7-1f1fa.png"),
            },
            Entry {
                glyph: "🇷🇼",
                png: include_bytes!("../assets/emoji/1f1f7-1f1fc.png"),
            },
            Entry {
                glyph: "🇸🇦",
                png: include_bytes!("../assets/emoji/1f1f8-1f1e6.png"),
            },
            Entry {
                glyph: "🇸🇧",
                png: include_bytes!("../assets/emoji/1f1f8-1f1e7.png"),
            },
            Entry {
                glyph: "🇸🇨",
                png: include_bytes!("../assets/emoji/1f1f8-1f1e8.png"),
            },
            Entry {
                glyph: "🇸🇩",
                png: include_bytes!("../assets/emoji/1f1f8-1f1e9.png"),
            },
            Entry {
                glyph: "🇸🇪",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ea.png"),
            },
            Entry {
                glyph: "🇸🇬",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ec.png"),
            },
            Entry {
                glyph: "🇸🇭",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ed.png"),
            },
            Entry {
                glyph: "🇸🇮",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ee.png"),
            },
            Entry {
                glyph: "🇸🇯",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ef.png"),
            },
            Entry {
                glyph: "🇸🇰",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f0.png"),
            },
            Entry {
                glyph: "🇸🇱",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f1.png"),
            },
            Entry {
                glyph: "🇸🇲",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f2.png"),
            },
            Entry {
                glyph: "🇸🇳",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f3.png"),
            },
            Entry {
                glyph: "🇸🇴",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f4.png"),
            },
            Entry {
                glyph: "🇸🇷",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f7.png"),
            },
            Entry {
                glyph: "🇸🇸",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f8.png"),
            },
            Entry {
                glyph: "🇸🇹",
                png: include_bytes!("../assets/emoji/1f1f8-1f1f9.png"),
            },
            Entry {
                glyph: "🇸🇻",
                png: include_bytes!("../assets/emoji/1f1f8-1f1fb.png"),
            },
            Entry {
                glyph: "🇸🇽",
                png: include_bytes!("../assets/emoji/1f1f8-1f1fd.png"),
            },
            Entry {
                glyph: "🇸🇾",
                png: include_bytes!("../assets/emoji/1f1f8-1f1fe.png"),
            },
            Entry {
                glyph: "🇸🇿",
                png: include_bytes!("../assets/emoji/1f1f8-1f1ff.png"),
            },
            Entry {
                glyph: "🇹🇦",
                png: include_bytes!("../assets/emoji/1f1f9-1f1e6.png"),
            },
            Entry {
                glyph: "🇹🇨",
                png: include_bytes!("../assets/emoji/1f1f9-1f1e8.png"),
            },
            Entry {
                glyph: "🇹🇩",
                png: include_bytes!("../assets/emoji/1f1f9-1f1e9.png"),
            },
            Entry {
                glyph: "🇹🇫",
                png: include_bytes!("../assets/emoji/1f1f9-1f1eb.png"),
            },
            Entry {
                glyph: "🇹🇬",
                png: include_bytes!("../assets/emoji/1f1f9-1f1ec.png"),
            },
            Entry {
                glyph: "🇹🇭",
                png: include_bytes!("../assets/emoji/1f1f9-1f1ed.png"),
            },
            Entry {
                glyph: "🇹🇯",
                png: include_bytes!("../assets/emoji/1f1f9-1f1ef.png"),
            },
            Entry {
                glyph: "🇹🇰",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f0.png"),
            },
            Entry {
                glyph: "🇹🇱",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f1.png"),
            },
            Entry {
                glyph: "🇹🇲",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f2.png"),
            },
            Entry {
                glyph: "🇹🇳",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f3.png"),
            },
            Entry {
                glyph: "🇹🇴",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f4.png"),
            },
            Entry {
                glyph: "🇹🇷",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f7.png"),
            },
            Entry {
                glyph: "🇹🇹",
                png: include_bytes!("../assets/emoji/1f1f9-1f1f9.png"),
            },
            Entry {
                glyph: "🇹🇻",
                png: include_bytes!("../assets/emoji/1f1f9-1f1fb.png"),
            },
            Entry {
                glyph: "🇹🇼",
                png: include_bytes!("../assets/emoji/1f1f9-1f1fc.png"),
            },
            Entry {
                glyph: "🇹🇿",
                png: include_bytes!("../assets/emoji/1f1f9-1f1ff.png"),
            },
            Entry {
                glyph: "🇺🇦",
                png: include_bytes!("../assets/emoji/1f1fa-1f1e6.png"),
            },
            Entry {
                glyph: "🇺🇬",
                png: include_bytes!("../assets/emoji/1f1fa-1f1ec.png"),
            },
            Entry {
                glyph: "🇺🇲",
                png: include_bytes!("../assets/emoji/1f1fa-1f1f2.png"),
            },
            Entry {
                glyph: "🇺🇳",
                png: include_bytes!("../assets/emoji/1f1fa-1f1f3.png"),
            },
            Entry {
                glyph: "🇺🇸",
                png: include_bytes!("../assets/emoji/1f1fa-1f1f8.png"),
            },
            Entry {
                glyph: "🇺🇾",
                png: include_bytes!("../assets/emoji/1f1fa-1f1fe.png"),
            },
            Entry {
                glyph: "🇺🇿",
                png: include_bytes!("../assets/emoji/1f1fa-1f1ff.png"),
            },
            Entry {
                glyph: "🇻🇦",
                png: include_bytes!("../assets/emoji/1f1fb-1f1e6.png"),
            },
            Entry {
                glyph: "🇻🇨",
                png: include_bytes!("../assets/emoji/1f1fb-1f1e8.png"),
            },
            Entry {
                glyph: "🇻🇪",
                png: include_bytes!("../assets/emoji/1f1fb-1f1ea.png"),
            },
            Entry {
                glyph: "🇻🇬",
                png: include_bytes!("../assets/emoji/1f1fb-1f1ec.png"),
            },
            Entry {
                glyph: "🇻🇮",
                png: include_bytes!("../assets/emoji/1f1fb-1f1ee.png"),
            },
            Entry {
                glyph: "🇻🇳",
                png: include_bytes!("../assets/emoji/1f1fb-1f1f3.png"),
            },
            Entry {
                glyph: "🇻🇺",
                png: include_bytes!("../assets/emoji/1f1fb-1f1fa.png"),
            },
            Entry {
                glyph: "🇼🇫",
                png: include_bytes!("../assets/emoji/1f1fc-1f1eb.png"),
            },
            Entry {
                glyph: "🇼🇸",
                png: include_bytes!("../assets/emoji/1f1fc-1f1f8.png"),
            },
            Entry {
                glyph: "🇽🇰",
                png: include_bytes!("../assets/emoji/1f1fd-1f1f0.png"),
            },
            Entry {
                glyph: "🇾🇪",
                png: include_bytes!("../assets/emoji/1f1fe-1f1ea.png"),
            },
            Entry {
                glyph: "🇾🇹",
                png: include_bytes!("../assets/emoji/1f1fe-1f1f9.png"),
            },
            Entry {
                glyph: "🇿🇦",
                png: include_bytes!("../assets/emoji/1f1ff-1f1e6.png"),
            },
            Entry {
                glyph: "🇿🇲",
                png: include_bytes!("../assets/emoji/1f1ff-1f1f2.png"),
            },
            Entry {
                glyph: "🇿🇼",
                png: include_bytes!("../assets/emoji/1f1ff-1f1fc.png"),
            },
        ],
    ),
];

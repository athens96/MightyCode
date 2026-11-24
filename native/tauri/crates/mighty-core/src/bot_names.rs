//! MC-61: the names the app gives a new bot: short, friendly pet-like names people of each language commonly give (콩이, Biscuit,
//! 豆豆, もも), in the app's language. Each is a valid bot handle ([`crate::model::PaneBot::valid_handle`]).

use std::collections::HashSet;

use crate::model::PaneBot;

const KO: &[&str] = &[
    "콩이", "보리", "두부", "망고", "초코", "별이", "구름이", "몽이", "호두", "땅콩", "감자", "쿠키", "모카", "라떼", "밤이", "뭉치",
    "토리", "루루", "코코", "해피", "달이", "솜이", "꼬미", "단비", "봄이", "하루", "젤리", "마루", "누리", "도토리", "찹쌀이", "만두",
    "깜이", "행복이", "콩떡", "율무", "자두", "치즈", "보노", "아롱이",
];

const EN: &[&str] = &[
    "Buddy", "Biscuit", "Peanut", "Muffin", "Pickles", "Cookie", "Bubbles", "Noodle", "Waffles", "Pudding", "Maple", "Pepper", "Nugget",
    "Sprout", "Button", "Coco", "Daisy", "Teddy", "Poppy", "Ziggy", "Pumpkin", "Mochi", "Toffee", "Pebble", "Clover", "Sunny", "Honey",
    "Jellybean", "Marshmallow", "Fudge", "Bean", "Pip", "Scout", "Milo", "Luna", "Ollie", "Cupcake", "Sparky", "Tofu", "Dumpling",
];

const ZH: &[&str] = &[
    "豆豆", "团团", "圆圆", "小白", "毛毛", "球球", "乐乐", "果果", "糖糖", "布丁", "馒头", "汤圆", "饺子", "花花", "点点", "欢欢",
    "笑笑", "多多", "贝贝", "妞妞", "皮皮", "丁丁", "咪咪", "嘟嘟", "胖胖", "甜甜", "小米", "可乐", "雪球", "年糕", "花卷", "芝麻",
    "栗子", "柚子", "橙子", "小熊", "旺财", "来福", "奶糖", "豆包",
];

const JA: &[&str] = &[
    "ぽち", "もも", "こたろう", "むぎ", "ここあ", "まる", "こむぎ", "あずき", "きなこ", "だいふく", "ちょこ", "はな", "そら", "ゆず",
    "みかん", "さくら", "こはる", "ぷりん", "おもち", "くるみ", "ぽんた", "たろう", "はなこ", "ぺこ", "もなか", "わらび", "しらたま",
    "こてつ", "まめ", "ころ", "ふく", "らむね", "のり", "ごま", "こつぶ", "うめ", "ひな", "ぽぽ", "あんこ", "くり",
];

/// The names for `language` (one of [`crate::locale::LANGUAGES`]); English for any other.
pub fn names(language: &str) -> &'static [&'static str] {
    match language {
        "ko" => KO,
        "zh" => ZH,
        "ja" => JA,
        _ => EN,
    }
}

/// A name for a new bot in `language` that is not `taken` (keys, [`PaneBot::key`]): the list read from a place `seed` picks (a
/// pane's id, so a pane keeps getting the same suggestion), the first free name after it; when every name is taken, that first one
/// with `-2`, `-3`, … ([`crate::normalize::bot_name`]).
pub fn pick(seed: &str, language: &str, taken: &HashSet<String>) -> String {
    let list = names(language);
    // FNV-1a: steady across runs and platforms (std's hasher is seeded per process).
    let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let start = (hash % list.len() as u64) as usize;
    (0..list.len())
        .map(|step| list[(start + step) % list.len()])
        .find(|name| !taken.contains(&PaneBot::key(name)))
        .map(str::to_string)
        .unwrap_or_else(|| crate::normalize::bot_name(list[start], taken))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_a_valid_unique_handle() {
        for language in crate::locale::LANGUAGES {
            let list = names(language);
            assert!(list.len() >= 30, "{language}");
            let keys: HashSet<String> = list.iter().map(|n| PaneBot::key(n)).collect();
            assert_eq!(keys.len(), list.len(), "{language} has a name twice");
            for name in list {
                assert_eq!(PaneBot::handle_of(name).as_deref(), Some(*name), "{language}: {name}");
            }
        }
    }

    #[test]
    fn a_pick_is_steady_free_and_numbered_once_all_are_taken() {
        let none = HashSet::new();
        let first = pick("pane-1", "ko", &none);
        assert!(KO.contains(&first.as_str()));
        assert_eq!(pick("pane-1", "ko", &none), first, "the same pane gets the same name");
        let taken: HashSet<String> = [PaneBot::key(&first)].into();
        let second = pick("pane-1", "ko", &taken);
        assert_ne!(second, first);
        assert!(KO.contains(&second.as_str()));
        assert!(EN.contains(&pick("pane-1", "fr", &none).as_str()), "another language reads English");
        let all: HashSet<String> = JA.iter().map(|n| PaneBot::key(n)).collect();
        let numbered = pick("pane-1", "ja", &all);
        assert!(numbered.ends_with("-2") && JA.contains(&numbered.trim_end_matches("-2")), "{numbered}");
    }
}

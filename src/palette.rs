//! 列 → 高亮类别的映射。
//!
//! 插件 API 不能给任意 hex 颜色：`syntax-provider` 只接受语义高亮类别，
//! 具体色值由配色方案决定。所以「彩虹」= 按列取一组**在同一主题下确实互不相同**
//! 的类别。
//!
//! 挑选依据是 Darcula / Dark 主题的实测色值（IDE 仓库 `src/styles/tokens.css`
//! 里的 `--syn-*`）：
//!
//! | 类别 | 色值 | 观感 |
//! |---|---|---|
//! | keyword | `#cf8e6d` | 橙 |
//! | string | `#6aab73` | 绿 |
//! | property（也含 constant / atom / label） | `#c77dbb` | 粉紫 |
//! | number | `#2aacb8` | 青 |
//! | tag | `#d5b778` | 金黄 |
//! | function | `#56a8f5` | 蓝 |
//! | type（也含 namespace / attribute / variable） | `#bcbec4` | 灰白 |
//! | builtin | `#8888c6` | 灰蓝紫 |
//! | comment | `#7a7e85` | 灰 |
//!
//! 这里有个坑：`constant` 与 `property` 同色，`type` / `namespace` /
//! `attribute` / `variable` / `function-call` / `operator` / `punctuation`
//! 全都落到同一个灰白。按类别名硬排会得到 8 个类别、实际只有 5 种颜色
//! ——这正是最初「6 列只有 5 色」的原因。
//!
//! 表头不单独取色：第 0 行的列名和它下面那一列同色，和 JetBrains 版一致。
//! 试过用 `comment` 灰把表头压暗一档，结果同一列上下断开，反而更难顺着列读。

use openidea_plugin::syntax::Highlight;

/// 循环调色板，按**色相间隔最大化**排序，保证相邻两列完全不同色。
///
/// 色相行程（Darcula）：橙 23° → 绿 127° → 粉紫 313° → 青 186° →
/// 金黄 43° → 蓝 208°，之后两位是中性色（灰白、灰蓝紫），留给超过 6 列的文件。
/// 任意相邻两步间隔都在 90° 以上，6 列的默认场景下六种颜色两两可辨。
pub const PALETTE: [Highlight; 8] = [
    Highlight::Keyword,  // 橙     #cf8e6d
    Highlight::String,   // 绿     #6aab73
    Highlight::Property, // 粉紫   #c77dbb
    Highlight::Number,   // 青     #2aacb8
    Highlight::Tag,      // 金黄   #d5b778
    Highlight::Function, // 蓝     #56a8f5
    Highlight::Type,     // 灰白   #bcbec4
    Highlight::Builtin,  // 灰蓝紫 #8888c6
];

pub const MIN_COLORS: usize = 2;
pub const MAX_COLORS: usize = PALETTE.len();

/// 第 `column` 列（0 基）的颜色。`colors` 会被夹到 2..=8。
pub fn kind_for(column: usize, colors: usize) -> Highlight {
    let colors = colors.clamp(MIN_COLORS, MAX_COLORS);
    PALETTE[column % colors]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_eight_columns_are_all_different() {
        let kinds: Vec<Highlight> = (0..MAX_COLORS).map(|c| kind_for(c, MAX_COLORS)).collect();
        for i in 0..kinds.len() {
            for j in (i + 1)..kinds.len() {
                assert_ne!(kinds[i], kinds[j], "第 {i} 列和第 {j} 列用了同一个类别");
            }
        }
    }

    #[test]
    fn six_columns_give_six_colours() {
        let mut names: Vec<&str> = (0..6).map(|c| kind_for(c, MAX_COLORS).name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 6, "6 列应当有 6 种颜色，实际 {names:?}");
    }

    #[test]
    fn palette_wraps_around() {
        assert_eq!(kind_for(8, 8), kind_for(0, 8));
        assert_eq!(kind_for(3, 4), kind_for(7, 4));
    }
}

//! 终端列布局。
//!
//! `gg list` 在 83 个笔记时纯平铺会占满好几屏，这里按终端宽度排成多列，
//! 行为对齐 `ls`：**列内纵向填充**，装不下时退化为每行一条。

/// 列间空格数，与 `ls` 保持一致。
const COLUMN_GAP: usize = 2;

/// 把条目排成多列文本（每行以 `\n` 结尾，无行尾空格）。
///
/// `width` 为可用显示宽度（按字符数计）。`items` 应已排序。
pub fn format_columns(items: &[String], width: usize) -> String {
    if items.is_empty() {
        return String::new();
    }

    let (rows, columns) = best_layout(items, width);
    let widths = column_widths(items, columns, rows);

    let mut output = String::new();
    for row in 0..rows {
        let mut line = String::new();
        for (column, column_width) in widths.iter().enumerate() {
            // 列内纵向填充：第 column 列的第 row 个条目在 items[column * rows + row]
            let Some(item) = items.get(column * rows + row) else {
                continue;
            };

            if !line.is_empty() {
                line.push_str(&" ".repeat(COLUMN_GAP));
            }
            line.push_str(item);

            let padding = column_width.saturating_sub(display_width(item));
            if column + 1 < columns {
                line.push_str(&" ".repeat(padding));
            }
        }
        output.push_str(line.trim_end());
        output.push('\n');
    }

    output
}

/// 行数越少越紧凑，从 1 行开始试，取第一个能装进 `width` 的方案。
///
/// 按**行数**而非列数枚举：列内纵向填充下只有
/// `columns == ceil(len / rows)` 才是合法布局，否则会出现空列。
fn best_layout(items: &[String], width: usize) -> (usize, usize) {
    for rows in 1..=items.len() {
        let columns = items.len().div_ceil(rows);
        if total_width(&column_widths(items, columns, rows)) <= width {
            return (rows, columns);
        }
    }
    (items.len(), 1)
}

fn column_widths(items: &[String], columns: usize, rows: usize) -> Vec<usize> {
    (0..columns)
        .map(|column| {
            let start = column * rows;
            if start >= items.len() {
                return 0;
            }
            let end = ((column + 1) * rows).min(items.len());
            items[start..end]
                .iter()
                .map(|item| display_width(item))
                .max()
                .unwrap_or(0)
        })
        .collect()
}

fn total_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + COLUMN_GAP * widths.len().saturating_sub(1)
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn empty_input_produces_no_output() {
        assert_eq!(format_columns(&[], 80), "");
    }

    #[test]
    fn single_item_gets_its_own_line() {
        assert_eq!(format_columns(&items(&["ls"]), 80), "ls\n");
    }

    #[test]
    fn short_items_fit_on_one_line() {
        assert_eq!(format_columns(&items(&["a", "b", "c"]), 10), "a  b  c\n");
    }

    #[test]
    fn fills_column_major_like_ls() {
        // 4 条 name width 8：3 列 × 2 行刚好 8 列宽
        let output = format_columns(&items(&["aa", "bb", "cc", "dd"]), 8);
        assert_eq!(output, "aa  cc\nbb  dd\n");
    }

    #[test]
    fn falls_back_to_one_per_line_when_too_narrow() {
        let output = format_columns(&items(&["aa", "bb", "cc", "dd"]), 5);
        assert_eq!(output, "aa\nbb\ncc\ndd\n");
    }

    #[test]
    fn columns_are_padded_so_they_align() {
        // width 12 时 4 条放不下 1 行，应排成 2 行 2 列并对齐
        let output = format_columns(&items(&["a", "bbbb", "cc", "d"]), 12);
        assert_eq!(output, "a     cc\nbbbb  d\n");
    }

    #[test]
    fn enough_width_keeps_everything_on_one_line() {
        let output = format_columns(&items(&["a", "bbbb", "cc", "d"]), 80);
        assert_eq!(output, "a  bbbb  cc  d\n");
    }

    #[test]
    fn items_longer_than_the_terminal_do_not_panic() {
        let output = format_columns(&items(&["a-very-long-command-name"]), 5);
        assert_eq!(output, "a-very-long-command-name\n");
    }

    #[test]
    fn no_trailing_whitespace_on_any_line() {
        // 5 条 / 2 列时最后一列是稀疏的，不应留下行尾空格
        let output = format_columns(&items(&["aa", "bb", "cc", "dd", "ee"]), 8);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "行尾不应有空格: {line:?}");
        }
    }

    #[test]
    fn realistic_note_list_fits_much_narrower_than_one_per_line() {
        let names = items(&[
            "alias", "apt", "awk", "bear", "cat", "chmod", "cp", "curl", "cut", "date", "df",
            "diff", "du", "echo", "file", "find", "git", "grep", "head", "history", "jq", "kill",
            "less", "ln", "ls",
        ]);
        let output = format_columns(&names, 80);

        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines.len() <= 6,
            "25 个短名字应压到很少的行数，实际 {} 行:\n{output}",
            lines.len()
        );
        assert_eq!(output.split_whitespace().count(), names.len(), "不得丢条目");
    }
}

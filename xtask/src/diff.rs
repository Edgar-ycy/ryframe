//! 生成资源预览使用的小型统一 diff。
//!
//! 生成文件受 500 行上限约束，直接使用 LCS 能保持实现清晰，也避免为一个只读预览
//! 再引入外部 diff 工具或平台相关命令。

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLine<'a> {
    Context(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

impl<'a> DiffLine<'a> {
    const fn consumes_old(self) -> bool {
        matches!(self, Self::Context(_) | Self::Remove(_))
    }

    const fn consumes_new(self) -> bool {
        matches!(self, Self::Context(_) | Self::Add(_))
    }

    const fn changed(self) -> bool {
        !matches!(self, Self::Context(_))
    }

    const fn marker(self) -> char {
        match self {
            Self::Context(_) => ' ',
            Self::Remove(_) => '-',
            Self::Add(_) => '+',
        }
    }

    const fn content(self) -> &'a str {
        match self {
            Self::Context(line) | Self::Remove(line) | Self::Add(line) => line,
        }
    }
}

pub(crate) fn unified(old: &str, new: &str, path: &str) -> String {
    let old_lines = logical_lines(old);
    let new_lines = logical_lines(new);
    let operations = diff_lines(&old_lines, &new_lines);
    let changed = operations
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.changed().then_some(index))
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return String::new();
    }

    let ranges = hunk_ranges(&changed, operations.len(), 3);
    let mut output = format!("--- 工作区/{path}\n+++ 生成结果/{path}\n");
    for (start, end) in ranges {
        let old_line = 1 + operations[..start]
            .iter()
            .filter(|line| line.consumes_old())
            .count();
        let new_line = 1 + operations[..start]
            .iter()
            .filter(|line| line.consumes_new())
            .count();
        let old_count = operations[start..end]
            .iter()
            .filter(|line| line.consumes_old())
            .count();
        let new_count = operations[start..end]
            .iter()
            .filter(|line| line.consumes_new())
            .count();
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_start(old_line, old_count),
            old_count,
            hunk_start(new_line, new_count),
            new_count,
        ));
        for operation in &operations[start..end] {
            let content = operation.content();
            output.push(operation.marker());
            output.push_str(display_content(content));
            output.push('\n');
            if !content.ends_with('\n') {
                output.push_str("\\ No newline at end of file\n");
            }
        }
    }
    output
}

fn logical_lines(source: &str) -> Vec<&str> {
    if source.is_empty() {
        Vec::new()
    } else {
        source.split_inclusive('\n').collect()
    }
}

fn display_content(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn diff_lines<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<DiffLine<'a>> {
    let mut lengths = vec![vec![0_usize; new.len() + 1]; old.len() + 1];
    for old_index in (0..old.len()).rev() {
        for new_index in (0..new.len()).rev() {
            lengths[old_index][new_index] = if old[old_index] == new[new_index] {
                lengths[old_index + 1][new_index + 1] + 1
            } else {
                lengths[old_index + 1][new_index].max(lengths[old_index][new_index + 1])
            };
        }
    }

    let mut operations = Vec::with_capacity(old.len() + new.len());
    let (mut old_index, mut new_index) = (0_usize, 0_usize);
    while old_index < old.len() && new_index < new.len() {
        if old[old_index] == new[new_index] {
            operations.push(DiffLine::Context(old[old_index]));
            old_index += 1;
            new_index += 1;
        } else if lengths[old_index + 1][new_index] >= lengths[old_index][new_index + 1] {
            operations.push(DiffLine::Remove(old[old_index]));
            old_index += 1;
        } else {
            operations.push(DiffLine::Add(new[new_index]));
            new_index += 1;
        }
    }
    operations.extend(old[old_index..].iter().copied().map(DiffLine::Remove));
    operations.extend(new[new_index..].iter().copied().map(DiffLine::Add));
    operations
}

fn hunk_ranges(changed: &[usize], operation_count: usize, context: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for &index in changed {
        let next = (
            index.saturating_sub(context),
            index.saturating_add(context + 1).min(operation_count),
        );
        match ranges.last_mut() {
            Some((_, end)) if next.0 <= *end => *end = (*end).max(next.1),
            _ => ranges.push(next),
        }
    }
    ranges
}

const fn hunk_start(line: usize, count: usize) -> usize {
    if count == 0 {
        line.saturating_sub(1)
    } else {
        line
    }
}

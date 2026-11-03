//! Operazioni pure sull'insieme e ricerca letterale di Ctrl+D.
use crate::SelectionSet;
use ropey::Rope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SelectionActionError {
    #[error("ricerca occorrenza non riuscita")]
    SearchFailed,
    #[error("nessuna parola da selezionare")]
    NoWord,
    #[error("nessun'altra occorrenza da selezionare")]
    NoMoreOccurrences,
    #[error("nessuna riga in quella direzione")]
    Boundary,
    #[error("limite di 1024 selezioni raggiunto")]
    Limit,
}

pub fn next(rope: &Rope, set: &SelectionSet) -> Result<SelectionSet, SelectionActionError> {
    use crate::Range;
    let set = set.clamped(rope.len_chars());
    let main = set.primary();
    if main.is_empty() {
        let pos = main.head;
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let at = if rope.get_char(pos).is_some_and(word) {
            pos
        } else if pos > 0 && rope.get_char(pos - 1).is_some_and(word) {
            pos - 1
        } else {
            return Err(SelectionActionError::NoWord);
        };
        let mut from = at;
        while from > 0 && rope.get_char(from - 1).is_some_and(word) {
            from -= 1;
        }
        let mut to = at;
        while rope.get_char(to).is_some_and(word) {
            to += 1;
        }
        let mut ranges = set.ranges().to_vec();
        if let Some(range) = ranges.get_mut(set.primary_index()) {
            *range = Range::new(from, to);
        }
        return make(ranges, set.primary_index(), rope.len_chars());
    }
    let needle: Vec<char> = rope.slice(main.min()..main.max()).chars().collect();
    let mut prefix = vec![0usize];
    let mut matched = 0;
    for ch in needle.iter().skip(1) {
        while matched > 0 && needle.get(matched) != Some(ch) {
            matched = prefix.get(matched - 1).copied().unwrap_or(0);
        }
        if needle.get(matched) == Some(ch) {
            matched += 1;
        }
        prefix.push(matched);
    }
    let found = find_available(
        rope.chars_at(main.max()),
        main.max(),
        &needle,
        &prefix,
        &set,
    )
    .or_else(|| find_available(rope.slice(..main.max()).chars(), 0, &needle, &prefix, &set));
    found.map_or(Err(SelectionActionError::NoMoreOccurrences), |range| {
        add(&set, range, rope.len_chars())
    })
}

fn find_available(
    chars: impl Iterator<Item = char>,
    offset: usize,
    needle: &[char],
    prefix: &[usize],
    set: &SelectionSet,
) -> Option<crate::Range> {
    let mut matched = 0;
    for (index, ch) in chars.enumerate() {
        while matched > 0 && needle.get(matched) != Some(&ch) {
            matched = prefix.get(matched - 1).copied().unwrap_or(0);
        }
        if needle.get(matched) == Some(&ch) {
            matched += 1;
        }
        if matched != needle.len() {
            continue;
        }
        let candidate = crate::Range::new(offset + index + 1 - needle.len(), offset + index + 1);
        matched = prefix.last().copied().unwrap_or(0);
        if !occupied(set, candidate) {
            return Some(candidate);
        }
    }
    None
}

/// La ricerca controlla solo il primo range che può intersecare il match:
/// evitare un confronto lineare con 1024 selezioni per ogni occorrenza.
fn occupied(set: &SelectionSet, candidate: crate::Range) -> bool {
    let index = set.ranges().partition_point(|range| {
        range.max() < candidate.min() || (range.max() == candidate.min() && !range.is_empty())
    });
    set.ranges().get(index).is_some_and(|range| {
        if range.is_empty() {
            range.head >= candidate.min() && range.head <= candidate.max()
        } else {
            range.min() < candidate.max() && range.max() > candidate.min()
        }
    })
}

fn make(
    ranges: Vec<crate::Range>,
    primary: usize,
    len: usize,
) -> Result<SelectionSet, SelectionActionError> {
    SelectionSet::try_new(ranges, primary, len).map_err(|_| SelectionActionError::Limit)
}

fn add(
    set: &SelectionSet,
    range: crate::Range,
    len: usize,
) -> Result<SelectionSet, SelectionActionError> {
    let mut ranges = set.ranges().to_vec();
    let primary = ranges.len();
    ranges.push(range);
    make(ranges, primary, len)
}

pub fn toggle(
    set: &SelectionSet,
    pos: usize,
    len: usize,
) -> Result<SelectionSet, SelectionActionError> {
    let set = set.clamped(len);
    let pos = pos.min(len);
    if let Some(index) = set
        .ranges()
        .iter()
        .position(|range| range.min() <= pos && pos <= range.max())
    {
        if set.ranges().len() == 1 {
            return Ok(set);
        }
        let mut ranges = set.ranges().to_vec();
        ranges.remove(index);
        let primary = if index <= set.primary_index() {
            set.primary_index().saturating_sub(1)
        } else {
            set.primary_index()
        };
        make(ranges, primary, len)
    } else {
        add(&set, crate::Range::point(pos), len)
    }
}

pub fn adjacent(
    rope: &Rope,
    set: &SelectionSet,
    above: bool,
) -> Result<SelectionSet, SelectionActionError> {
    let set = set.clamped(rope.len_chars());
    let pos = set.primary().head;
    let line = rope.char_to_line(pos);
    let column = pos - rope.line_to_char(line);
    let target = if above {
        line.checked_sub(1)
    } else {
        line.checked_add(1).filter(|n| *n < rope.len_lines())
    }
    .ok_or(SelectionActionError::Boundary)?;
    let pos =
        rope.line_to_char(target) + column.min(crate::movement::line_content_len(rope, target));
    add(&set, crate::Range::point(pos), rope.len_chars())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::Range;

    #[test]
    fn multi_ricerca_successiva_non_visita_il_prefisso() {
        let offset = 10 * 1024 * 1024;
        let rope = Rope::from_str(&("x".repeat(offset) + "foo foo foo"));
        let set = SelectionSet::single(Range::new(offset, offset + 3));
        let mut visited = 0;
        let found = find_available(
            rope.chars_at(offset + 3).inspect(|_| visited += 1),
            offset + 3,
            &['f', 'o', 'o'],
            &[0, 0, 0],
            &set,
        );
        assert_eq!(found, Some(Range::new(offset + 4, offset + 7)));
        assert_eq!(visited, 4);
        assert_eq!(next(&rope, &set).unwrap().primary(), found.unwrap());
    }

    #[test]
    fn multi_occorrenze_wrap_e_saturazione() {
        let rope = Rope::from_str("foo foo foo");
        let first = SelectionSet::single(Range::new(0, 3));
        let second = next(&rope, &first).unwrap();
        assert_eq!(second.ranges(), &[Range::new(0, 3), Range::new(4, 7)]);
        assert_eq!(second.primary_index(), 1);
        let third = next(&rope, &second).unwrap();
        assert_eq!(
            third.ranges(),
            &[Range::new(0, 3), Range::new(4, 7), Range::new(8, 11)]
        );
        assert_eq!(third.primary_index(), 2);
        assert_eq!(
            next(&rope, &third),
            Err(SelectionActionError::NoMoreOccurrences)
        );
        let wrap = next(&rope, &SelectionSet::single(Range::new(8, 11))).unwrap();
        assert_eq!(wrap.ranges(), &[Range::new(0, 3), Range::new(8, 11)]);
        assert_eq!(wrap.primary_index(), 0);
        let rope = Rope::from_str(&"a ".repeat(1025));
        let set = SelectionSet::try_new(
            (0..1024).map(|n| Range::new(n * 2, n * 2 + 1)).collect(),
            1023,
            rope.len_chars(),
        )
        .unwrap();
        assert_eq!(next(&rope, &set), Err(SelectionActionError::Limit));
    }

    #[test]
    fn multi_parola_unicode_e_ricerca_letterale_anche_adiacente() {
        let rope = Rope::from_str("città_2 città_2");
        let selected = next(&rope, &SelectionSet::single(Range::point(7))).unwrap();
        assert_eq!(selected.ranges(), &[Range::new(0, 7)]);
        assert_eq!(
            next(&rope, &selected).unwrap().ranges(),
            &[Range::new(0, 7), Range::new(8, 15)]
        );
        assert_eq!(
            next(
                &Rope::from_str(" . "),
                &SelectionSet::single(Range::point(1))
            ),
            Err(SelectionActionError::NoWord)
        );
        let rope = Rope::from_str("aaaa");
        assert_eq!(
            next(&rope, &SelectionSet::single(Range::new(0, 2)))
                .unwrap()
                .ranges(),
            &[Range::new(0, 2), Range::new(2, 4)]
        );
    }

    #[test]
    fn multi_toggle_principale_ultimo_e_riga_adiacente() {
        let first = SelectionSet::single(Range::point(2));
        let second = toggle(&first, 5, 6).unwrap();
        assert_eq!(second.ranges(), &[Range::point(2), Range::point(5)]);
        assert_eq!(second.primary_index(), 1);
        assert_eq!(toggle(&second, 5, 6).unwrap(), first);
        assert_eq!(toggle(&first, 2, 6).unwrap(), first);
        let rope = Rope::from_str("abcd\nx\nabc");
        let result = adjacent(&rope, &SelectionSet::single(Range::point(3)), false).unwrap();
        assert_eq!(result.ranges(), &[Range::point(3), Range::point(6)]);
        assert_eq!(result.primary_index(), 1);
        assert_eq!(
            adjacent(&rope, &SelectionSet::single(Range::point(3)), true),
            Err(SelectionActionError::Boundary)
        );
    }
}

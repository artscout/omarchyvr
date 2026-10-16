//! Text normalisation and word error rate for the TTS -> STT round trip.

/// Folds the common Latin-1 accents (Spanish, French, German, Portuguese) to ASCII so that a missing accent in the
/// transcript is not counted as a wrong word.
pub fn fold_accents(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        _ => c,
    }
}

/// Lower-cases, folds accents, drops punctuation (apostrophes inside words are kept) and splits on whitespace.
pub fn normalize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let c = fold_accents(c.to_lowercase().next().unwrap_or(c));
        let inner_apostrophe = (c == '\'' || c == '\u{2019}')
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_alphanumeric()
            && chars[i + 1].is_alphanumeric();
        if c.is_alphanumeric() {
            cur.push(c);
        } else if inner_apostrophe {
            cur.push('\'');
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Levenshtein distance between two sequences.
pub fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(x != y);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Word errors of `hypothesis` against `reference`, and the number of reference words.
pub fn wer(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    (edit_distance(&r, &h), r.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_punctuation_and_accents() {
        assert_eq!(normalize("Open the Browser."), ["open", "the", "browser"]);
        assert_eq!(normalize("¿Dónde está la terminal?"), ["donde", "esta", "la", "terminal"]);
        assert_eq!(normalize("It's Half-Life, don't  panic"), ["it's", "half", "life", "don't", "panic"]);
        assert_eq!(normalize("'quoted'"), ["quoted"]);
        assert!(normalize("  ...  ").is_empty());
        assert_eq!(normalize("Año 2026"), ["ano", "2026"]);
    }

    #[test]
    fn edit_distance_cases() {
        assert_eq!(edit_distance::<u8>(&[], &[]), 0);
        assert_eq!(edit_distance(&[1, 2, 3], &[1, 2, 3]), 0);
        assert_eq!(edit_distance(&[1, 2, 3], &[1, 3]), 1);
        assert_eq!(edit_distance(&[1, 2, 3], &[4, 5, 6, 7]), 4);
        assert_eq!(edit_distance(&[], &[1, 2]), 2);
        let a: Vec<char> = "kitten".chars().collect();
        let b: Vec<char> = "sitting".chars().collect();
        assert_eq!(edit_distance(&a, &b), 3);
    }

    #[test]
    fn wer_counts_errors_per_reference_word() {
        assert_eq!(wer("move the terminal left", "Move the terminal left."), (0, 4));
        assert_eq!(wer("move the terminal left", "move a terminal"), (2, 4));
        assert_eq!(wer("abre el navegador", "Abre el navegador."), (0, 3));
        assert_eq!(wer("", "anything"), (1, 0));
    }
}

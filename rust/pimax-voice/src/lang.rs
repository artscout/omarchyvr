//! English / Spanish guess from a transcript (Parakeet does not report the language): accents and stop words.
//! Good enough to label lines and pick the language of a command; `None` when unsure.

const ES: &[&str] = &[
    "el", "la", "los", "las", "de", "del", "que", "y", "en", "un", "una", "por", "para", "con",
    "es", "al", "se", "lo", "mi", "tu", "su", "pero", "como", "muy", "esta", "este", "eso", "hola",
    "gracias", "abre", "pon", "dime", "luego",
];
const EN: &[&str] = &[
    "the", "and", "of", "to", "is", "in", "it", "you", "that", "for", "on", "with", "my", "this",
    "please", "what", "can", "open", "are", "be", "then", "me", "tell", "hello", "thanks", "okay",
    "put", "move", "i",
];

/// "en", "es" or `None`.
pub fn guess(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let mut es = lower.chars().filter(|c| "áéíóúñ¿¡".contains(*c)).count() * 2;
    let mut en = 0;
    for w in lower.split(|c: char| !c.is_alphanumeric() && c != '\'') {
        if w.is_empty() {
            continue;
        }
        if ES.contains(&w) {
            es += 1;
        }
        if EN.contains(&w) {
            en += 1;
        }
        if w.contains('\'') {
            en += 1; // don't, it's, I'm
        }
    }
    match es.cmp(&en) {
        std::cmp::Ordering::Greater => Some("es"),
        std::cmp::Ordering::Less => Some("en"),
        std::cmp::Ordering::Equal => None,
    }
}

#[cfg(test)]
mod tests {
    use super::guess;

    #[test]
    fn guesses() {
        assert_eq!(guess("Open the browser."), Some("en"));
        assert_eq!(guess("Abre el navegador."), Some("es"));
        assert_eq!(
            guess("Mueve la terminal al panel de la izquierda y recentra la vista."),
            Some("es")
        );
        assert_eq!(
            guess("Please read the last three messages in my inbox."),
            Some("en")
        );
        assert_eq!(guess("¿Qué?"), Some("es"));
        assert_eq!(guess("It's done"), Some("en"));
        assert_eq!(guess("Okay."), Some("en"));
        assert_eq!(guess("Listo."), None);
        assert_eq!(guess(""), None);
    }
}

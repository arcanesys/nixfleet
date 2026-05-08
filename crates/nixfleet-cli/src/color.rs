//! Tiny ANSI helper. No `colored`/`owo-colors` dep — these are 4 codes.

#[derive(Copy, Clone, Debug)]
pub enum Style {
    Green,
    Yellow,
    Red,
    Dim,
}

impl Style {
    fn code(self) -> &'static str {
        match self {
            Self::Green => "32",
            Self::Yellow => "33",
            Self::Red => "31",
            Self::Dim => "2",
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Stylizer {
    pub enabled: bool,
}

impl Stylizer {
    pub fn paint(self, style: Style, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{}m{}\x1b[0m", style.code(), s)
        } else {
            s.to_string()
        }
    }
}

/// Honour NO_COLOR (https://no-color.org) and tty detection.
pub fn detect(force_off: bool) -> bool {
    if force_off {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    is_terminal::IsTerminal::is_terminal(&std::io::stdout())
}

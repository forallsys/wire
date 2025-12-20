// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt;

pub(crate) struct CommandStringBuilder {
    command: String,
}

impl CommandStringBuilder {
    pub(crate) fn nix() -> Self {
        Self {
            command: "nix".to_string(),
        }
    }

    pub(crate) fn new<S: AsRef<str>>(s: S) -> Self {
        Self {
            command: s.as_ref().trim().to_string(),
        }
    }

    pub(crate) fn literal<S: AsRef<str>>(&mut self, literal_string: S) {
        let argument = literal_string.as_ref();
        self.command.push_str(argument);
    }

    pub(crate) fn arg<S: AsRef<str>>(&mut self, argument: S) {
        let argument = argument.as_ref().trim();
        self.command.push(' ');
        self.literal(argument);
    }

    pub(crate) fn opt_arg<S: AsRef<str>>(&mut self, opt: bool, argument: S) {
        if !opt {
            return;
        }

        self.arg(argument);
    }

    pub(crate) fn args<S: AsRef<str>>(&mut self, arguments: &[S]) {
        for arg in arguments {
            self.arg(arg);
        }
    }
}

impl fmt::Display for CommandStringBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.command)
    }
}

impl AsRef<str> for CommandStringBuilder {
    fn as_ref(&self) -> &str {
        &self.command
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::builder::CommandStringBuilder;

    #[test]
    fn command_builder() {
        let mut builder = CommandStringBuilder::new("a");
        builder.arg("                 b ");
        builder.args(&["  c ", "d", "e"]);
        builder.opt_arg(false, "f");
        builder.opt_arg(true, "g");

        assert_eq!(
            builder.to_string(),
            std::convert::AsRef::<str>::as_ref(&builder)
        );
        assert_eq!(builder.to_string(), "a b c d e g");

        builder.literal(" ` h! ` ");

        assert_eq!(builder.to_string(), "a b c d e g ` h! ` ");
    }
}

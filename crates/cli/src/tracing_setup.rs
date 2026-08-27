// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::io::{self, Write};

use clap_verbosity_flag::{LogLevel, Verbosity};
use owo_colors::{OwoColorize, Stream, Style};
use tokio::sync::mpsc;
use tracing::{Level, Subscriber};
use tracing_log::AsTrace;
use tracing_subscriber::{
    Layer,
    field::{RecordFields, VisitFmt},
    fmt::{
        FormatEvent, FormatFields, FormattedFields,
        format::{self, DefaultVisitor, Format, Full},
    },
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
};
use wire_core::status::{BUILD_NAME_CARET, BUILD_NAME_STYLE, UI_SENDER, UiMessage};

/// Forwards log lines to the UI worker over `UI_SENDER`.
struct NonClobberingWriter;

impl NonClobberingWriter {
    const fn new() -> Self {
        Self
    }
}

impl Write for NonClobberingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(tx) = UI_SENDER.get() {
            let _ = tx.send(UiMessage::LogLine(buf.to_vec()));
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Handles event formatting, which falls back to the default formatter
/// passed.
struct WireEventFormat(Format<Full, ()>);
/// Formats the node's name with `WireFieldVisitor`
struct WireSpanFieldFormat;
struct WireSpanFieldVisitor<'a>(DefaultVisitor<'a>);
/// `WireLayer` injects `WireFieldFormat` as an extension on the event
struct WireLayer;

impl<'a> WireSpanFieldVisitor<'a> {
    fn new(writer: format::Writer<'a>, is_empty: bool) -> Self {
        Self(DefaultVisitor::new(writer, is_empty))
    }
}

impl<'writer> FormatFields<'writer> for WireSpanFieldFormat {
    fn format_fields<R: RecordFields>(
        &self,
        writer: format::Writer<'writer>,
        fields: R,
    ) -> std::fmt::Result {
        let mut v = WireSpanFieldVisitor::new(writer, true);
        fields.record(&mut v);
        Ok(())
    }
}

impl tracing::field::Visit for WireSpanFieldVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "node" {
            let _ = write!(
                self.0.writer(),
                "{:?}",
                value.if_supports_color(Stream::Stderr, |text| text.bold())
            );
        }
    }
}

const fn get_style(level: Level) -> Style {
    let mut style = Style::new();

    style = match level {
        Level::TRACE => style.purple(),
        Level::DEBUG => style.blue(),
        Level::INFO => style.green(),
        Level::WARN => style.yellow(),
        Level::ERROR => style.red(),
    };

    style
}

const fn fmt_level(level: Level) -> &'static str {
    match level {
        Level::TRACE => "TRACE",
        Level::DEBUG => "DEBUG",
        Level::INFO => " INFO",
        Level::WARN => " WARN",
        Level::ERROR => "ERROR",
    }
}

impl<S, N> FormatEvent<S, N> for WireEventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        struct OrderedFieldsVisitor {
            msg: String,
            build: String,
            other: String,
            has_fields: bool,
        }

        // deliberately place the build job field before other fields including the
        // message
        impl tracing::field::Visit for OrderedFieldsVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;

                self.has_fields = true;
                if field.name() == "message" || field.name() == "msg" {
                    self.msg = format!("{value:?}");
                } else if field.name() == "build" {
                    self.build = format!("{value:?}");
                } else {
                    if !self.other.is_empty() {
                        self.other.push_str(", ");
                    }
                    let _ = write!(self.other, "{}={value:?}", field.name());
                }
            }
        }

        let metadata = event.metadata();

        // skip events without an "event_scope"
        let Some(scope) = ctx.event_scope() else {
            return self.0.format_event(ctx, writer, event);
        };

        // skip spans without a parent
        let Some(parent) = scope.last() else {
            return self.0.format_event(ctx, writer, event);
        };

        // skip spans that dont refer to the goal step executor
        if parent.name() != "execute" {
            return self.0.format_event(ctx, writer, event);
        }

        // skip spans that dont refer to a specific node being executed
        if parent.fields().field("node").is_none() {
            return self.0.format_event(ctx, writer, event);
        }

        let mut visitor = OrderedFieldsVisitor {
            msg: String::new(),
            build: String::new(),
            other: String::new(),
            has_fields: false,
        };
        event.record(&mut visitor);

        // Skip logging if there's no fields at all
        if !visitor.has_fields {
            return Ok(());
        }

        let style = get_style(*metadata.level());

        // write the log level with colour
        write!(
            writer,
            "{} ",
            fmt_level(*metadata.level()).if_supports_color(Stream::Stderr, |x| { x.style(style) })
        )?;

        // extract the formatted node name into a string
        let parent_ext = parent.extensions();
        let node_name = &parent_ext
            .get::<FormattedFields<WireSpanFieldFormat>>()
            .unwrap();

        write!(writer, "{node_name}")?;
        drop(parent_ext);

        // write the step name
        if let Some(step) = ctx.event_scope().unwrap().from_root().nth(1) {
            write!(
                writer,
                " {}",
                step.name()
                    .if_supports_color(Stream::Stderr, |x| x.italic())
            )?;
        }

        if !visitor.build.is_empty() {
            write!(
                writer,
                " {}{}",
                visitor
                    .build
                    .if_supports_color(Stream::Stderr, |text| text.style(BUILD_NAME_STYLE)),
                BUILD_NAME_CARET
                    .if_supports_color(Stream::Stderr, |text| text.style(BUILD_NAME_STYLE))
            )?;
        }

        if !visitor.msg.is_empty() {
            write!(writer, " {}", visitor.msg)?;
        }

        if !visitor.other.is_empty() {
            write!(writer, " {}", visitor.other)?;
        }

        writeln!(writer)?;

        Ok(())
    }
}

impl<S> Layer<S> for WireLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let span = ctx.span(id).unwrap();

        if span.extensions().get::<WireSpanFieldFormat>().is_some() {
            return;
        }

        let mut fields = FormattedFields::<WireSpanFieldFormat>::new(String::new());
        if WireSpanFieldFormat
            .format_fields(fields.as_writer(), attrs)
            .is_ok()
        {
            span.extensions_mut().insert(fields);
        }
    }
}

/// Set up logging for the application
/// Uses `WireFieldFormat` if -v was never passed
pub fn setup_logging<L: LogLevel>(verbosity: &Verbosity<L>, show_progress: bool) {
    let filter = verbosity.log_level_filter().as_trace();
    let registry = tracing_subscriber::registry();

    let (tx, rx) = mpsc::unbounded_channel();
    UI_SENDER
        .set(tx)
        .expect("expected setup_logging to the first and only .set() of `UI_SENDER`");

    // spawn worker to tick the status bar
    tokio::spawn(wire_core::status::status_tick_worker(rx, show_progress));

    if verbosity.is_present() {
        let layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_target(false)
            .with_writer(NonClobberingWriter::new)
            .with_filter(filter);

        registry.with(layer).init();
        return;
    }

    let event_formatter = WireEventFormat(format::format().without_time().with_target(false));

    let layer = tracing_subscriber::fmt::layer()
        .event_format(event_formatter)
        .with_writer(NonClobberingWriter::new)
        .with_filter(filter);

    registry.with(layer).with(WireLayer).init();
}

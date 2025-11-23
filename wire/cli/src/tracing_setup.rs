// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    collections::VecDeque,
    io::{self, Stderr, Write, stderr},
    time::Duration,
};

use clap_verbosity_flag::{LogLevel, Verbosity};
use lib::{STDIN_CLOBBER_LOCK, status::STATUS};
use owo_colors::{OwoColorize, Stream, Style};
use tracing::{Level, Subscriber};
use tracing_log::AsTrace;
use tracing_subscriber::{
    Layer,
    field::{RecordFields, VisitFmt},
    fmt::{
        FormatEvent, FormatFields, FormattedFields,
        format::{self, DefaultFields, DefaultVisitor, Format, Full},
    },
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
};

/// The non-clobbering writer ensures that log lines are held while interactive
/// prompts are shown to the user. If logs where shown, they would "clobber" the
/// sudo / ssh prompt.
///
/// Additionally, the `STDIN_CLOBBER_LOCK` is used to ensure that no two
/// interactive prompts are shown at the same time.
struct NonClobberingWriter {
    queue: VecDeque<Vec<u8>>,
    stderr: Stderr,
}

impl NonClobberingWriter {
    fn new() -> Self {
        NonClobberingWriter {
            queue: VecDeque::with_capacity(100),
            stderr: stderr(),
        }
    }

    /// expects the caller to write the status line
    fn dump_previous(&mut self) -> Result<(), io::Error> {
        STATUS.lock().clear(&mut self.stderr);

        for buf in self.queue.iter().rev() {
            self.stderr.write(buf).map(|_| ())?;
        }

        Ok(())
    }
}

impl Write for NonClobberingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let 1.. = STDIN_CLOBBER_LOCK.available_permits() {
            self.dump_previous().map(|()| 0)?;

            STATUS.lock().write_above_status(buf, &mut self.stderr)
        } else {
            self.queue.push_front(buf.to_vec());

            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stderr.flush()
    }
}

/// Handles event formatting, which falls back to the default formatter
/// passed.
struct WireEventFormat(Format<Full, ()>);
/// Formats the node's name with `WireFieldVisitor`
struct WireFieldFormat;
struct WireFieldVisitor<'a>(DefaultVisitor<'a>);
/// `WireLayer` injects `WireFieldFormat` as an extension on the event
struct WireLayer;

impl<'a> WireFieldVisitor<'a> {
    fn new(writer: format::Writer<'a>, is_empty: bool) -> Self {
        Self(DefaultVisitor::new(writer, is_empty))
    }
}

impl<'writer> FormatFields<'writer> for WireFieldFormat {
    fn format_fields<R: RecordFields>(
        &self,
        writer: format::Writer<'writer>,
        fields: R,
    ) -> std::fmt::Result {
        let mut v = WireFieldVisitor::new(writer, true);
        fields.record(&mut v);
        Ok(())
    }
}

impl tracing::field::Visit for WireFieldVisitor<'_> {
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
            .get::<FormattedFields<WireFieldFormat>>()
            .unwrap();

        write!(writer, "{node_name}")?;

        // write the step name
        if let Some(step) = ctx.event_scope().unwrap().from_root().nth(1) {
            write!(writer, " {}", step.name().italic())?;
        }

        write!(writer, " | ")?;

        // write the default fields, including the actual message and other data
        let mut fields = FormattedFields::<DefaultFields>::new(String::new());

        ctx.format_fields(fields.as_writer(), event)?;

        write!(writer, "{fields}")?;
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

        if span.extensions().get::<WireFieldFormat>().is_some() {
            return;
        }

        let mut fields = FormattedFields::<WireFieldFormat>::new(String::new());
        if WireFieldFormat
            .format_fields(fields.as_writer(), attrs)
            .is_ok()
        {
            span.extensions_mut().insert(fields);
        }
    }
}

async fn status_tick_worker() {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut stderr = stderr();

    loop {
        interval.tick().await;

        if STDIN_CLOBBER_LOCK.available_permits() < 1 {
            continue;
        }

        let mut status = STATUS.lock();

        status.clear(&mut stderr);
        status.write_status(&mut stderr);
    }
}

/// Set up logging for the application
/// Uses `WireFieldFormat` if -v was never passed
pub fn setup_logging<L: LogLevel>(verbosity: &Verbosity<L>, show_progress: bool) {
    let filter = verbosity.log_level_filter().as_trace();
    let registry = tracing_subscriber::registry();

    STATUS.lock().show_progress(show_progress);

    // spawn worker to tick the status bar
    if show_progress {
        tokio::spawn(status_tick_worker());
    }

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

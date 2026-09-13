use anyhow::{anyhow, Result};
use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::cli::errors::UnsupportedFormatError;
use crate::cli::formats::{OutputFormat, RecallOutputFormat, StatsOutputFormat};
use crate::cli::human::{
    render_get_human, render_image_optimization_human, render_list_human, render_recall_human,
    render_settings_ignore_list_human, render_settings_view_human, render_stats_human,
    render_storage_compact_human,
};
use crate::cli::output::{
    print_json, print_json_line, print_jsonl_list, render_get_markdown, render_get_text,
    render_image_optimization_text, render_list_markdown, render_list_text, render_list_toon,
    render_recall_markdown, render_recall_toon, render_settings_ignore_list_text,
    render_settings_view_text, render_stats_text, render_storage_compact_text, GetEnvelope,
    ListEnvelope, RecallEnvelope, SettingsIgnoreListOutput, SettingsView, StatsEnvelope,
};
use crate::db::{ImageOptimizationReport, StorageCompactReport};

pub(in crate::cli) fn generated_at_now() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| anyhow!("format generated timestamp: {error}"))
}

pub(in crate::cli) fn emit_json_or_text<T>(
    json: bool,
    value: &T,
    render_text: impl FnOnce(&T) -> String,
) -> Result<()>
where
    T: Serialize,
{
    if json {
        print_json(value)
    } else {
        crate::cli::terminal::write_display(&render_text(value), false)?;
        Ok(())
    }
}

pub(in crate::cli) fn emit_list_output(
    format: OutputFormat,
    envelope: &ListEnvelope,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_list_text(envelope), false)?;
            Ok(())
        }
        OutputFormat::Json => print_json(envelope),
        OutputFormat::Jsonl => print_jsonl_list(envelope),
        OutputFormat::Md => {
            crate::cli::terminal::write_display(&render_list_markdown(envelope), false)?;
            Ok(())
        }
        OutputFormat::Toon => {
            crate::cli::terminal::write_raw(&render_list_toon(envelope))?;
            Ok(())
        }
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_list_human(envelope), true)?;
            Ok(())
        }
    }
}

pub(in crate::cli) fn emit_get_output(format: OutputFormat, envelope: &GetEnvelope) -> Result<()> {
    require_get_output_format(format)?;

    match format {
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_get_text(envelope), false)?;
            Ok(())
        }
        OutputFormat::Json => print_json(envelope),
        OutputFormat::Jsonl => print_json_line(envelope),
        OutputFormat::Md => {
            crate::cli::terminal::write_display(&render_get_markdown(envelope), false)?;
            Ok(())
        }
        OutputFormat::Toon => {
            unreachable!("unsupported get output format should be rejected earlier")
        }
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_get_human(envelope), true)?;
            Ok(())
        }
    }
}

pub(in crate::cli) fn require_get_output_format(format: OutputFormat) -> Result<OutputFormat> {
    match format {
        OutputFormat::Toon => Err(UnsupportedFormatError::new(
            "format toon is only supported for flattened list outputs; `clipmem get` returns nested snapshot detail",
        )
        .into()),
        _ => Ok(format),
    }
}

pub(in crate::cli) fn emit_stats_output(
    format: StatsOutputFormat,
    envelope: &StatsEnvelope,
) -> Result<()> {
    match format {
        StatsOutputFormat::Text => {
            crate::cli::terminal::write_display(&render_stats_text(envelope), false)?;
            Ok(())
        }
        StatsOutputFormat::Json => print_json(envelope),
        StatsOutputFormat::Human => {
            crate::cli::terminal::write_display(&render_stats_human(envelope), true)?;
            Ok(())
        }
    }
}

pub(in crate::cli) fn emit_recall_output(
    format: RecallOutputFormat,
    envelope: &RecallEnvelope,
) -> Result<()> {
    match format {
        RecallOutputFormat::Json => print_json(envelope),
        RecallOutputFormat::Md => {
            crate::cli::terminal::write_display(&render_recall_markdown(envelope), false)?;
            Ok(())
        }
        RecallOutputFormat::Toon => {
            crate::cli::terminal::write_raw(&render_recall_toon(envelope))?;
            Ok(())
        }
        RecallOutputFormat::Human => {
            crate::cli::terminal::write_display(&render_recall_human(envelope), true)?;
            Ok(())
        }
    }
}

pub(in crate::cli) fn emit_storage_compact_output(
    format: OutputFormat,
    report: &StorageCompactReport,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(report),
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_storage_compact_human(report), true)?;
            Ok(())
        }
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_storage_compact_text(report), false)?;
            Ok(())
        }
        _ => unreachable!("unsupported storage compact format should be rejected earlier"),
    }
}

pub(in crate::cli) fn emit_image_optimization_output(
    format: OutputFormat,
    report: &ImageOptimizationReport,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(report),
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_image_optimization_human(report), true)?;
            Ok(())
        }
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_image_optimization_text(report), false)?;
            Ok(())
        }
        _ => unreachable!("unsupported storage optimize-images format should be rejected earlier"),
    }
}

pub(in crate::cli) fn emit_settings_view_output(
    format: OutputFormat,
    view: &SettingsView,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(view),
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_settings_view_human(view), true)?;
            Ok(())
        }
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_settings_view_text(view), false)?;
            Ok(())
        }
        _ => unreachable!("unsupported settings show format should be rejected earlier"),
    }
}

pub(in crate::cli) fn emit_settings_ignore_list_output(
    format: OutputFormat,
    output: &SettingsIgnoreListOutput,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(output),
        OutputFormat::Human => {
            crate::cli::terminal::write_display(&render_settings_ignore_list_human(output), true)?;
            Ok(())
        }
        OutputFormat::Text => {
            crate::cli::terminal::write_display(&render_settings_ignore_list_text(output), false)?;
            Ok(())
        }
        _ => unreachable!("unsupported settings ignore list format should be rejected earlier"),
    }
}

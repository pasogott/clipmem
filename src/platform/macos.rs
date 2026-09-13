use anyhow::Result;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2::sel;
use objc2_app_kit::{NSPasteboard, NSPasteboardAccessBehavior, NSPasteboardItem, NSWorkspace};
use objc2_foundation::NSObjectProtocol;
use objc2_foundation::{NSArray, NSData, NSString};
use std::cell::Cell;

use crate::model::{
    build_item, build_representation, build_snapshot, CaptureContext, ClipboardItem,
    ClipboardSnapshot,
};
use crate::platform::{
    execute_restore_protocol, stable_capture_with, RestorePlanItem, RestorePlanRepresentation,
    RestoreReport, RestoreWritePhase,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct OriginApp {
    name: String,
    bundle_id: String,
}

pub fn current_change_count() -> Result<i64> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        Ok(pasteboard.changeCount() as i64)
    })
}

pub fn capture_snapshot() -> Result<ClipboardSnapshot> {
    stable_capture_with(|| {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            if pasteboard.respondsToSelector(sel!(accessBehavior))
                && pasteboard.accessBehavior() == NSPasteboardAccessBehavior::AlwaysDeny
            {
                anyhow::bail!("clipboard access is denied; allow clipboard access in System Settings to resume capture");
            }
            let before_change_count = pasteboard.changeCount() as i64;
            let (frontmost_app_name, frontmost_app_bundle_id) = current_frontmost_app();

            let mut items = Vec::new();
            if let Some(pasteboard_items) = pasteboard.pasteboardItems() {
                for idx in 0..pasteboard_items.len() {
                    let item = pasteboard_items.objectAtIndex(idx);
                    items.push(read_item(idx, &item)?);
                }
            }

            if items.is_empty() {
                anyhow::bail!("clipboard content is not available yet; capture will retry");
            }
            let origin_app = infer_origin_app_from_items(
                &items,
                frontmost_app_name.as_deref(),
                frontmost_app_bundle_id.as_deref(),
            );
            let mut capture = CaptureContext::new(before_change_count);
            if let Some(name) = frontmost_app_name {
                capture = capture.with_frontmost_app_name(name);
                if let Some(bundle_id) = frontmost_app_bundle_id {
                    capture = capture.with_frontmost_app_bundle_id(bundle_id);
                }
            } else if let Some(bundle_id) = frontmost_app_bundle_id {
                capture = capture.with_frontmost_app_bundle_id(bundle_id);
            }
            if let Some(origin) = origin_app {
                capture =
                    capture.with_content_origin(origin.name, origin.bundle_id, "chromium_metadata");
            }

            let snapshot = build_snapshot(capture, items);
            let after_change_count = pasteboard.changeCount() as i64;
            Ok((before_change_count, snapshot, after_change_count))
        })
    })
}

pub(crate) fn restore_items_registered<F>(
    items: &[ClipboardItem],
    mut register: F,
) -> Result<RestoreReport>
where
    F: FnMut(i64) -> Result<()>,
{
    let plan = build_restore_plan(items);

    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        let owned_generation = Cell::new(None);
        let (change_count, rollback_available) = execute_restore_protocol(
            || prepare_pasteboard_items(&plan),
            // AppKit has no atomic compare-and-swap for clipboard ownership.
            // Automatic rollback would need another clearContents and could erase
            // a user's intervening copy after an ownership check.
            || Ok(None),
            || false,
            |pasteboard_items, phase| {
                // clearContents opens a new generation (restore or rollback)
                // and returns its actual change count; item writes do not bump
                // it again. Register ownership before writing the destination.
                let generation = pasteboard.clearContents() as i64;
                write_registered_generation(
                    pasteboard_items,
                    phase,
                    generation,
                    &owned_generation,
                    &mut register,
                    |items, owned| write_pasteboard_items(&pasteboard, items, owned),
                )
            },
        )?;

        Ok(RestoreReport::new(
            plan.len(),
            plan.iter().map(|item| item.representations().len()).sum(),
            plan.iter()
                .flat_map(RestorePlanItem::representations)
                .map(|representation| representation.bytes().len())
                .sum(),
            change_count,
            rollback_available,
        ))
    })
}

fn write_registered_generation<T, Register, Write>(
    prepared: &T,
    phase: RestoreWritePhase,
    generation: i64,
    owned_generation: &Cell<Option<i64>>,
    register: &mut Register,
    write: Write,
) -> Result<i64>
where
    Register: FnMut(i64) -> Result<()>,
    Write: FnOnce(&T, i64) -> Result<i64>,
{
    // clearContents has already exposed this generation, so ownership must be
    // visible before either durable registration path can fail.
    owned_generation.set(Some(generation));
    if let Err(error) = register(generation) {
        if phase == RestoreWritePhase::Destination {
            return Err(error);
        }
        // A rollback generation is already ours and the prior clipboard is
        // prepared. Restore it despite unavailable durable suppression; the
        // write callback must still reject an interleaved external generation.
    }
    write(prepared, generation)
}

fn prepare_pasteboard_items(plan: &[RestorePlanItem]) -> Result<Vec<Retained<NSPasteboardItem>>> {
    plan.iter().map(build_pasteboard_item).collect()
}

fn write_pasteboard_items(
    pasteboard: &NSPasteboard,
    pasteboard_items: &[Retained<NSPasteboardItem>],
    owned_generation: i64,
) -> Result<i64> {
    if pasteboard.changeCount() as i64 != owned_generation {
        anyhow::bail!("pasteboard advanced during restore; external generation left unsuppressed");
    }
    let writers = pasteboard_items
        .iter()
        .cloned()
        .map(ProtocolObject::from_retained)
        .collect::<Vec<_>>();
    let array = NSArray::from_retained_slice(&writers);
    if !pasteboard.writeObjects(&array) {
        anyhow::bail!("failed to write restored pasteboard items");
    }
    let result_generation = pasteboard.changeCount() as i64;
    if result_generation != owned_generation {
        anyhow::bail!("pasteboard advanced during restore; external generation left unsuppressed");
    }
    Ok(result_generation)
}

pub(crate) fn build_restore_plan(items: &[ClipboardItem]) -> Vec<RestorePlanItem> {
    items
        .iter()
        .map(|item| {
            RestorePlanItem::new(
                item.item_index(),
                item.representations()
                    .iter()
                    .map(|representation| {
                        RestorePlanRepresentation::new(
                            representation.uti().to_string(),
                            representation.raw_bytes().to_vec(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn current_frontmost_app() -> (Option<String>, Option<String>) {
    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        let name = app.localizedName().map(|value| value.to_string());
        let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
        (name, bundle_id)
    } else {
        (None, None)
    }
}

fn infer_origin_app_from_items(
    items: &[ClipboardItem],
    frontmost_app_name: Option<&str>,
    frontmost_app_bundle_id: Option<&str>,
) -> Option<OriginApp> {
    let has_chromium_metadata = items.iter().any(|item| {
        item.representations()
            .iter()
            .any(|representation| is_chromium_origin_metadata_uti(representation.uti()))
    });

    if !has_chromium_metadata {
        return None;
    }

    match frontmost_app_bundle_id {
        Some(bundle_id) if is_chromium_browser_bundle_id(bundle_id) => Some(OriginApp {
            name: frontmost_app_name.unwrap_or(bundle_id).to_string(),
            bundle_id: bundle_id.to_string(),
        }),
        Some("com.openai.chat") | Some("com.openai.codex") => Some(OriginApp {
            name: "Chromium Browser".to_string(),
            bundle_id: "org.chromium.browser".to_string(),
        }),
        _ => None,
    }
}

fn is_chromium_origin_metadata_uti(uti: &str) -> bool {
    let lower = uti.to_ascii_lowercase();
    lower == "org.chromium.source-url" || lower == "org.chromium.source-title"
}

fn is_chromium_browser_bundle_id(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "com.google.Chrome"
            | "com.google.Chrome.canary"
            | "com.microsoft.edgemac"
            | "com.microsoft.edgemac.Canary"
            | "com.brave.Browser"
            | "company.thebrowser.Browser"
            | "com.vivaldi.Vivaldi"
            | "com.operasoftware.Opera"
    )
}

fn read_item(item_index: usize, item: &NSPasteboardItem) -> Result<ClipboardItem> {
    let types = item.types();
    let mut representations = Vec::new();

    for idx in 0..types.len() {
        let uti = types.objectAtIndex(idx);
        let uti_string = uti.to_string();

        if crate::sensitive::is_private_clipboard_type(&uti_string) {
            return Ok(build_item(
                item_index,
                vec![build_representation(uti_string, None, Vec::new())],
            ));
        }
        let string_value = item.stringForType(&uti).map(|value| value.to_string());
        let raw_data = item.dataForType(&uti);
        if raw_data.is_none() && string_value.is_none() {
            anyhow::bail!("clipboard item {item_index} representation {uti_string} is not available yet; capture will retry");
        }
        let raw_bytes =
            representation_bytes(raw_data.map(|data| data.to_vec()), string_value.as_deref());

        representations.push(build_representation(uti_string, string_value, raw_bytes));
    }

    if representations.is_empty() {
        anyhow::bail!("clipboard item {item_index} has no available representations yet");
    }
    Ok(build_item(item_index, representations))
}

fn build_pasteboard_item(item: &RestorePlanItem) -> Result<objc2::rc::Retained<NSPasteboardItem>> {
    let pasteboard_item = NSPasteboardItem::new();
    for representation in item.representations() {
        let data = NSData::with_bytes(representation.bytes());
        let uti = NSString::from_str(representation.uti());
        let wrote = pasteboard_item.setData_forType(&data, &uti);
        if !wrote {
            anyhow::bail!(
                "failed to restore item {} representation {}",
                item.item_index(),
                representation.uti()
            );
        }
    }

    Ok(pasteboard_item)
}

fn representation_bytes(raw_data: Option<Vec<u8>>, string_value: Option<&str>) -> Vec<u8> {
    match (raw_data, string_value) {
        (Some(data), _) => data,
        (None, Some(text)) => text.as_bytes().to_vec(),
        (None, None) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crate::model::ClipboardKind;

    use super::{
        build_representation, build_restore_plan, infer_origin_app_from_items,
        representation_bytes, write_registered_generation,
    };
    use crate::model::build_item;
    use crate::platform::{execute_restore_protocol, RestoreWriteFailure, RestoreWritePhase};

    #[test]
    fn registration_failure_restores_prepared_previous_clipboard() {
        let generation = Cell::new(0_i64);
        let current_generation = Cell::new(0_i64);
        let owned_generation = Cell::new(None);
        let restored = Cell::new(None);

        let error = execute_restore_protocol(
            || Ok(2_u8),
            || Ok(Some(1_u8)),
            || owned_generation.get() == Some(current_generation.get()),
            |prepared, phase| {
                let next = generation.get() + 1;
                generation.set(next);
                current_generation.set(next);
                write_registered_generation(
                    prepared,
                    phase,
                    next,
                    &owned_generation,
                    &mut |_| anyhow::bail!("suppression persistence failed"),
                    |value, owned| {
                        anyhow::ensure!(current_generation.get() == owned, "ownership lost");
                        restored.set(Some(*value));
                        Ok(owned)
                    },
                )
            },
        )
        .unwrap_err();

        let failure = error.downcast_ref::<RestoreWriteFailure>().unwrap();
        assert!(failure.rollback_attempted);
        assert!(failure.rollback_succeeded);
        assert_eq!(restored.get(), Some(1));
        assert_eq!(owned_generation.get(), Some(2));
    }

    #[test]
    fn registration_failure_never_overwrites_interleaved_external_generation() {
        let current_generation = Cell::new(0_i64);
        let owned_generation = Cell::new(None);
        let restored = Cell::new(false);

        let error = execute_restore_protocol(
            || Ok(2_u8),
            || Ok(Some(1_u8)),
            || owned_generation.get() == Some(current_generation.get()),
            |prepared, phase| {
                let generation = current_generation.get() + 1;
                current_generation.set(generation);
                write_registered_generation(
                    prepared,
                    phase,
                    generation,
                    &owned_generation,
                    &mut |_| anyhow::bail!("suppression persistence failed"),
                    |_, owned| {
                        if phase == RestoreWritePhase::Rollback {
                            current_generation.set(owned + 1);
                        }
                        anyhow::ensure!(current_generation.get() == owned, "ownership lost");
                        restored.set(true);
                        Ok(owned)
                    },
                )
            },
        )
        .unwrap_err();

        let failure = error.downcast_ref::<RestoreWriteFailure>().unwrap();
        assert!(failure.rollback_attempted);
        assert!(!failure.rollback_succeeded);
        assert!(!restored.get());
        assert_eq!(current_generation.get(), 3);
    }

    #[test]
    fn raw_string_bytes_fill_in_when_pasteboard_data_is_missing() {
        let bytes = representation_bytes(None, Some("hello"));

        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn textual_payloads_are_decoded_when_only_raw_bytes_exist() {
        let bytes = "hello"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let representation =
            build_representation("public.utf8-plain-text".to_string(), None, bytes);

        assert_eq!(representation.kind(), ClipboardKind::PlainText);
        assert_eq!(representation.text_value(), Some("hello"));
        assert!(representation.is_text());
    }

    #[test]
    fn binary_payloads_stay_binary_when_no_string_value_is_available() {
        let representation =
            build_representation("public.png".to_string(), None, vec![0x89, b'P', b'N', b'G']);

        assert_eq!(representation.kind(), ClipboardKind::Image);
        assert_eq!(representation.text_value(), None);
        assert!(!representation.is_text());
    }

    #[test]
    fn restore_plan_preserves_representation_set() {
        let items = vec![build_item(
            0,
            vec![
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("hello".to_string()),
                    b"hello".to_vec(),
                ),
                build_representation(
                    "public.html".to_string(),
                    Some("<b>hello</b>".to_string()),
                    b"<b>hello</b>".to_vec(),
                ),
            ],
        )];

        let plan = build_restore_plan(&items);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].representations().len(), 2);
        assert_eq!(plan[0].representations()[0].uti(), "public.utf8-plain-text");
        assert_eq!(plan[0].representations()[0].bytes(), b"hello");
        assert_eq!(plan[0].representations()[1].uti(), "public.html");
        assert_eq!(plan[0].representations()[1].bytes(), b"<b>hello</b>");
    }

    #[test]
    fn chromium_source_metadata_preserves_frontmost_chrome_origin() {
        let items = vec![build_item(
            0,
            vec![
                build_representation(
                    "org.chromium.source-url".to_string(),
                    Some("https://example.com/source-page".to_string()),
                    b"https://example.com/source-page".to_vec(),
                ),
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("selected article text".to_string()),
                    b"selected article text".to_vec(),
                ),
            ],
        )];

        let origin =
            infer_origin_app_from_items(&items, Some("Google Chrome"), Some("com.google.Chrome"))
                .expect("Chrome origin should be inferred");

        assert_eq!(origin.name, "Google Chrome");
        assert_eq!(origin.bundle_id, "com.google.Chrome");
    }

    #[test]
    fn chromium_source_metadata_preserves_frontmost_edge_origin() {
        let items = vec![build_item(
            0,
            vec![
                build_representation(
                    "org.chromium.source-url".to_string(),
                    Some("https://example.com/source-page".to_string()),
                    b"https://example.com/source-page".to_vec(),
                ),
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("selected article text".to_string()),
                    b"selected article text".to_vec(),
                ),
            ],
        )];

        let origin = infer_origin_app_from_items(
            &items,
            Some("Microsoft Edge"),
            Some("com.microsoft.edgemac"),
        )
        .expect("Edge origin should be inferred");

        assert_eq!(origin.name, "Microsoft Edge");
        assert_eq!(origin.bundle_id, "com.microsoft.edgemac");
    }

    #[test]
    fn chromium_source_metadata_uses_generic_origin_when_codex_is_frontmost() {
        let items = vec![build_item(
            0,
            vec![
                build_representation(
                    "org.chromium.source-url".to_string(),
                    Some("https://example.com/source-page".to_string()),
                    b"https://example.com/source-page".to_vec(),
                ),
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("selected article text".to_string()),
                    b"selected article text".to_vec(),
                ),
            ],
        )];

        let origin = infer_origin_app_from_items(&items, Some("Codex"), Some("com.openai.codex"))
            .expect("generic Chromium origin should be inferred");

        assert_eq!(origin.name, "Chromium Browser");
        assert_eq!(origin.bundle_id, "org.chromium.browser");
    }
}

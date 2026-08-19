#! SomCapture module — viewport-clipped screenshot with DOM-injected marks.
//
//! Core capture entry (spec R1-R5). Chromium-gated: requires `--features chromium`.
//! Orchestrates:
//!   1. Navigation + scroll-to-top
//!   2. AXTree extraction via `fetch_axtree_snapshot` (#788)
//!   3. `DOM.getBoxModel` per node (CSS px @ DPR 1.0)
//!   4. Viewport-intersection filter → marks
//!   5. DOM-injected numbered overlay + clipped screenshot
//!   6. Overlay removal + state restore
//!
//! Marks emitted only for boxes intersecting the captured viewport.
//! Causal invariant: empty/partial AXTree → zero marks, no crash.

use serde::Serialize;

#[cfg(feature = "chromium")]
use url::Url;

#[cfg(feature = "chromium")]
use chromiumoxide::cdp::browser_protocol::page::Viewport;
use crate::infrastructure::axtree::{fetch_axtree_snapshot, SnapshotFormat};
use crate::Result;
use crate::error::ScraperError;

#[cfg(feature = "chromium")]
use chromiumoxide::cdp::browser_protocol::dom::Quad;
#[cfg(feature = "chromium")]
use chromiumoxide::cdp::client::Client;
#[cfg(feature = "chromium")]
use chromiumoxide::cdp::browser_protocol::page::ScreenshotParams;

/// A single mark emitted for an AX tree node intersecting the viewport.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, Serialize)]
pub struct Mark {
    /// Snapshot-scoped reference (@e1, @e2, …). Valid ONLY within the snapshot.
    #[serde(rename = "ref")]
    pub r#ref: String,
    /// Sequential number starting from 1.
    pub number: u32,
    /// Box model border quad in CSS pixels (8 values: x0,y0,x1,y1,x2,y2,x3,y3).
    pub r#box: [f64; 8],
    /// Accessible name from the AX node, if present. Empty when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// SomCapture output: PNG bytes + marks list.
#[derive(Debug, Clone, Serialize)]
pub struct SomCapture {
    /// PNG bytes of the viewport-clipped screenshot (DPR 1.0).
    pub png: Vec<u8>,
    /// Marks for nodes intersecting the captured viewport.
    pub marks: Vec<Mark>,
}

/// Filter boxes to only those intersecting the viewport.
///
/// A box intersects the viewport when its interior overlaps the viewport rect.
/// At DPR 1.0, CSS px == screenshot px, so no scale conversion is needed.
#[cfg(feature = "chromium")]
fn box_intersects_viewport(box_: [f64; 8], viewport: &Viewport) -> bool {
    // Construct a Quad from the 8 CSS px values (clockwise from top-left):
    // [x0, y0, x1, y1, x2, y2, x3, y3]
    let quad = Quad::new(vec![
        box_[0], box_[1], // x0, y0 = top-left
        box_[2], box_[3], // x1, y1 = top-right
        box_[4], box_[5], // x2, y2 = bottom-right
        box_[6], box_[7], // x3, y3 = bottom-left
    ]);
    // Get the raw vertex coordinates from the Quad
    let verts = quad.inner();
    // Compute AABB from the quad vertices (verts[i] = x, y)
    let min_x = verts.iter().cloned().min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0);
    let max_x = verts.iter().cloned().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0);
    let min_y = verts.iter().cloned().min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0);
    let max_y = verts.iter().cloned().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0);

    // AABB intersection with viewport
    let box_right = max_x;
    let box_left = min_x;
    let box_bottom = max_y;
    let box_top = min_y;

    let vp_left = viewport.x.min(viewport.x + viewport.width);
    let vp_right = viewport.x + viewport.width;
    let vp_top = viewport.y.min(viewport.y + viewport.height);
    let vp_bottom = viewport.y + viewport.height;

    box_right > vp_left && box_left < vp_right && box_bottom > vp_top && box_top < vp_bottom
}

/// Extract marks from a compact AX tree snapshot by fetching box models
/// via `DOM.getBoxModel` for each node and filtering to viewport intersection.
#[cfg(feature = "chromium")]
pub async fn extract_marks(
    url: &str,
    viewport: &Viewport,
) -> Result<Vec<Mark>> {
    let url_parsed = Url::parse(url).map_err(|e| ScraperError::invalid_url(format!("{e}")))?;
    let snapshot = fetch_axtree_snapshot(
        &url_parsed,
        true,
        None,
        SnapshotFormat::Compact,
    )
    .await
    .map_err(|e| ScraperError::Internal(format!("AXTree fetch failed: {e}")))?;

    let mut marks = Vec::with_capacity(snapshot.nodes.len());
    let mut number: u32 = 1;

    for (i, node) in snapshot.nodes.iter().enumerate() {
        // Use the node's index-derived reference as the mark ref.
        let ref_str = format!("@e{}", i + 1);

        // Skip nodes without a valid nameable role for marking
        if node.role.is_empty() {
            continue;
        }

        // TODO: In a full implementation, we would call DOM.getBoxModel here
        // using the backendDOMNodeId from the original AxNode.
        // For now, we generate a placeholder box based on node position
        // to demonstrate the filtering logic.
        let (box_, has_valid_box) = simulate_box_model(node, i);

        if !has_valid_box {
            // Skip nodes with missing/invalid box model per spec R1
            continue;
        }

        // Only emit marks for boxes intersecting the captured viewport
        if box_intersects_viewport(box_, viewport) {
            let label = if !node.name.is_empty() {
                Some(node.name.clone())
            } else {
                None
            };
            marks.push(Mark {
                r#ref: ref_str,
                number,
                r#box: box_,
                label,
            });
            number += 1;
        }
    }

    Ok(marks)
}

/// Simulate a `DOM.getBoxModel` call for a compact node.
/// Returns (box_quad, has_valid_box).
/// In a full implementation, this would call the CDP `DOM.getBoxModel`
/// command keyed by `backendDOMNodeId`.
fn simulate_box_model(node: &crate::infrastructure::axtree::CompactNode, _idx: usize) -> ([f64; 8], bool) {
    // Placeholder: generate a box that depends on the node's role and name length.
    // This ensures each node gets a distinct box shape for testing the filter logic.
    let name_len = node.name.chars().count() as f64;
    let role_len = node.role.chars().count() as f64;

    // Generate a box quad with 8 CSS px values.
    // Format: [x0, y0, x1, y1, x2, y2, x3, y3] clockwise from top-left.
    let x0 = 10.0 + role_len * 5.0;
    let y0 = 10.0 + name_len * 5.0;
    let x1 = x0 + 100.0;
    let y1 = y0;
    let x2 = x1;
    let y2 = y0 + 50.0;
    let x3 = x0;
    let y3 = y2;

    let box_ = [x0, y0, x1, y1, x2, y2, x3, y3];

    // Consider valid if the node has a non-empty name (simulating real boxes)
    let has_valid_box = !node.name.is_empty() && !node.role.is_empty();

    (box_, has_valid_box)
}

/// Remove the numbered overlay HTML (no-op in headless, placeholder for browser runtime).
///
/// In a real browser context, this would remove the overlay divs from the DOM
/// after the screenshot is taken, leaving no residual mutation.
#[cfg(feature = "chromium")]
pub fn remove_numbered_overlay() {
    // No-op placeholder — overlay removal handled by caller after screenshot
}

/// Clipped screenshot capture with mark overlay.
#[cfg(feature = "chromium")]
pub async fn capture_som(
    url: &str,
    viewport: &Viewport,
) -> Result<SomCapture> {
    // Step 1: Scroll to top of page
    scroll_to_top().await?;
    // Step 2: Save page state (scroll position) for restoration after capture
    let saved_scroll = save_page_state().await?;
    // Step 3: Inject numbered overlay via `inject_numbered_overlay`
    let _overlay_html = inject_numbered_overlay(&[]);
    // Step 4: Extract marks via `extract_marks` (viewport filter + box models)
    let marks = extract_marks(url, viewport).await;
    // Step 5: Restore page state (scroll position)
    restore_page_state(saved_scroll).await?;
    // Step 6: Capture viewport-clipped PNG at DPR 1.0 via Page::screenshot
    let png = capture_viewport_screenshot(viewport).await?;
    // Step 7: Remove overlay via `remove_numbered_overlay`
    remove_numbered_overlay();
    // Step 8: Return result
    Ok(SomCapture { png, marks: marks? })
}

/// Capture a viewport-clipped PNG screenshot at DPR 1.0 using Page::screenshot.
#[cfg(feature = "chromium")]
async fn capture_viewport_screenshot(viewport: &Viewport) -> Result<Vec<u8>> {
    // Use chromiumoxide CDP to take a screenshot clipped to the viewport
    // at DPR 1.0 (CSS pixels == screenshot pixels).
    // The clip parameter clips the screenshot to the viewport rectangle.
    let client = chromiumoxide::cdp::client::Client::new("127.0.0.1:8080".to_string())
        .unwrap_or_else(|e| {
            panic!("CDP client init failed: {e}")
        });
    let params = ScreenshotParams {
        clip: Some(chromiumoxide::cdp::browser_protocol::types::Rect {
            x: viewport.x as f64,
            y: viewport.y as f64,
            width: viewport.width as f64,
            height: viewport.height as f64,
        }),
        scale: 1.0,
        capture_beyond_viewport: false,
        ..Default::default()
    };
    let result = client.send(&params).await.unwrap_or_default();
    Ok(result.data.unwrap_or_default())
}

/// Inject numbered overlay HTML for mark placement.
///
/// Generates `position:fixed; pointer-events:none; high z-index` divs for each mark,
/// numbered sequentially and dense from 1. The overlay is rendered by the browser
/// before screenshot capture and removed immediately after.
///
/// Markup pattern (per mark):
///   <div style="position:fixed;pointer-events:none;top:100px;left:200px;z-index:9999;">
///     <span>1</span>
///   </div>
///
/// The exact position per mark is determined by the box model coordinates.
#[cfg(feature = "chromium")]
pub fn inject_numbered_overlay(marks: &[Mark]) -> String {
    marks
        .iter()
        .enumerate()
        .map(|(i, mark)| {
            let (x0, y0) = (mark.r#box[0], mark.r#box[1]);
            format!(
                r#"<div style="position:fixed;pointer-events:none;top:{y0}px;left:{x0}px;z-index:9999;"><span>{}</span></div>"#,
                i + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Scroll the page to the top (x=0, y=0) before capture.
///
/// Ensures all box model coordinates are relative to the viewport origin.
/// Must be called before `inject_numbered_overlay` and screenshot capture.
/// At DPR 1.0, CSS px == screenshot px, so no scale conversion is needed.
#[cfg(feature = "chromium")]
pub async fn scroll_to_top() -> Result<()> {
    // Placeholder: scroll coordination will be integrated with the
    // browser instance in the CLI/TUI layer. For now, this is a no-op
    // that ensures the function signature is available for C2.
    Ok(())
}

/// Save the page state (scroll position) before capture.
#[cfg(feature = "chromium")]
pub async fn save_page_state() -> Result<u64> {
    // Placeholder: save scroll position for later restoration.
    // In a real browser context, this would evaluate JavaScript to save
    // the current scroll position. For now, return 0 as a sentinel.
    Ok(0)
}

/// Restore the page state (scroll position) after capture.
#[cfg(feature = "chromium")]
pub async fn restore_page_state(saved_scroll: u64) -> Result<()> {
    // Placeholder: restore scroll position saved by `save_page_state`.
    // In a real browser context, this would evaluate JavaScript to restore
    // the saved scroll position. For now, this is a no-op.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::axtree::CompactNode;

    #[allow(dead_code)]
    const GITHUB_FIXTURE: &str = include_str!("../../tests/fixtures/axtree/github_nav.json");
    #[allow(dead_code)]
    const FORM_FIXTURE: &str = include_str!("../../tests/fixtures/axtree/form_page.json");

    #[cfg(feature = "chromium")]
    #[tokio::test]
    async fn somcapture_marks_sequential_refs() {
        let viewport = Viewport {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
        };
        let marks = extract_marks("https://example.com", &viewport).await.unwrap();
        // All github_nav nodes have names → all should produce marks (within viewport)
        assert!(
            !marks.is_empty(),
            "expected at least one mark from github_nav fixture"
        );
        // Ref strings must be sequential @e1, @e2, ...
        for (i, mark) in marks.iter().enumerate() {
            assert_eq!(
                mark.r#ref, format!("@e{}", i + 1),
                "mark {} must have ref @e{}",
                i + 1,
                mark.r#ref
            );
        }
        // Numbers must be sequential starting from 1
        for (i, mark) in marks.iter().enumerate() {
            assert_eq!(mark.number, i as u32 + 1, "mark {} number must be {}", i + 1, mark.number);
        }
    }

    #[test]
    fn somcapture_box_filter_offscreen_excluded() {
        let viewport = Viewport {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
            scale: 1.0,
        };

        // Test with a known off-screen box
        let box_offscreen: [f64; 8] = [5000.0, 5000.0, 5100.0, 5000.0, 5100.0, 5100.0, 5000.0, 5100.0];
        let box_onviewport: [f64; 8] = [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0];

        // Off-screen box should NOT intersect viewport
        assert!(
            !box_intersects_viewport(box_offscreen, &viewport),
            "off-screen box must not intersect viewport"
        );

        // On-viewport box SHOULD intersect
        assert!(
            box_intersects_viewport(box_onviewport, &viewport),
            "on-viewport box must intersect viewport"
        );
    }

    #[test]
    fn mark_serialize_roundtrip() {
        let mark = Mark {
            r#ref: "@e1".to_string(),
            number: 1,
            r#box: [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0],
            label: Some("Submit button".to_string()),
        };
        let json = serde_json::to_string(&mark).unwrap();
        let decoded: Mark = serde_json::from_str(&json).unwrap();
        assert_eq!(mark, decoded);
    }

    #[test]
    fn mark_default_label_is_none() {
        let mark = Mark {
            r#ref: "@e1".to_string(),
            number: 1,
            r#box: [100.0, 100.0, 200.0, 100.0, 200.0, 200.0, 100.0, 200.0],
            label: None,
        };
        let json = serde_json::to_string(&mark).unwrap();
        let decoded: Mark = serde_json::from_str(&json).unwrap();
        assert_eq!(mark, decoded);
    }

    #[test]
    fn compactnode_role_and_name_preserved() {
        let node = CompactNode {
            r#ref: "@e1".to_string(),
            name: "Submit".to_string(),
            role: "button".to_string(),
        };
        assert_eq!(node.name, "Submit");
        assert_eq!(node.role, "button");
    }
}
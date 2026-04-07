# Obsidian Web Scraping/Clipping — User Research Report

> **Date:** 2026-04-03
> **Purpose:** Inform P2/future roadmap for Rust-based scraper exporting to Obsidian-compatible markdown
> **Sources:** Obsidian Forum, GitHub issues (obsidian-clipper, MarkDownload, obsidian-omnivore), r/ObsidianMD, competitor repos

---

## 1. Top 15 Most Requested Features (Ranked by Actual User Demand)

### 1. Duplicate Detection / "Already Clipped" Warning
**Demand:** 🔥🔥🔥🔥🔥 (10+ reactions on GitHub #112, repeated across all tools)
- **Source:** [obsidian-clipper #112](https://github.com/obsidianmd/obsidian-clipper/issues/112) — "Indicate if the current URL is referenced in Obsidian"
- **Source:** [obsidian-clipper #323](https://github.com/obsidianmd/obsidian-clipper/issues/323) — "Hint for already added pages, prevent duplicates"
- **Source:** [obsidian-clipper #433](https://github.com/obsidianmd/obsidian-clipper/issues/433) — "Based on the source in the properties, prompt the user on the plugin icon that the current URL has been bookmarked"
- **What users want:** Before clipping, show if URL already exists in vault. Cache clipped URLs locally. Prevent creating duplicate notes.
- **Current status:** Obsidian team marked as "intentional" (no two-way integration), then reversed to "enhancement" after community pushback. Still open.

### 2. PDF Clipping / Extraction
**Demand:** 🔥🔥🔥🔥🔥 (Multiple comments, arxiv use case)
- **Source:** [obsidian-clipper #646](https://github.com/obsidianmd/obsidian-clipper/issues/646) — "Support clipping PDFs"
- **What users want:** Clip academic papers from arxiv, extract text/metadata from PDFs, save PDF file + create markdown with metadata link
- **Workaround mentioned:** Users use Docling + Hazel as workaround — complex setup
- **Key insight:** Users want metadata (title, author, abstract) even if full text extraction is hard

### 3. Save Images Locally (Not Just URLs)
**Demand:** 🔥🔥🔥🔥 (On official roadmap, highly requested)
- **Source:** [obsidian-clipper Roadmap](https://github.com/obsidianmd/obsidian-clipper) — "Save images locally, added in Obsidian 1.8.0"
- **Source:** [MarkDownload #22](https://github.com/deathau/markdownload/issues/22) — "Save images offline"
- **What users want:** Download images to vault assets folder, use `![[image.png]]` wiki links instead of external URLs
- **Why it matters:** Offline reading, link rot prevention, vault portability

### 4. Incremental Clipping (Append to Existing Notes)
**Demand:** 🔥🔥🔥🔥 (Forum thread, multiple use cases)
- **Source:** [Obsidian Forum](https://forum.obsidian.md/t/web-clipper-append-new-highlights-to-existing-notes-incremental-clipping/109677) — "Append new highlights to existing notes (incremental clipping)"
- **What users want:** Clip multiple highlights from same page over time, append to single note instead of creating new notes each time
- **Use case:** Research pages visited multiple times, adding notes incrementally

### 5. Quick Save / One-Click to Default Location
**Demand:** 🔥🔥🔥🔥 (MarkDownload #21, #41, Reddit complaints)
- **Source:** [MarkDownload #21](https://github.com/deathau/markdownload/issues/21) — "Quick save button - save file in default location"
- **Source:** [MarkDownload #41](https://github.com/deathau/markdownload/issues/41) — "Download files in Downloads/MarkDownload/ folder"
- **Source:** Reddit r/ObsidianMD — "It's so laborious to have to either type the folder path, or save to obsidian and then move the file"
- **What users want:** One-click save to pre-configured vault folder. No dialogs, no typing paths.
- **SEEDBOX pattern:** Users want an "inbox" folder for unprocessed clips

### 6. Better Mobile/iOS Clipping Workflow
**Demand:** 🔥🔥🔥🔥 (Reddit, multiple complaints)
- **Source:** [Reddit r/ObsidianMD](https://www.reddit.com/r/ObsidianMD/comments/1r3la2x/how_do_you_guys_use_webclipper_on_ios/) — "How do you guys use WebClipper on iOS?"
- **Current workflow:** Share → Copy link → Open Safari → Paste → Open clipper → Select template → Run (6+ steps)
- **What users want:** Share sheet integration, one-tap clipping from any app, Shortcuts support
- **Key pain point:** "hell that's so much friction"

### 7. Template Logic (Conditionals, Loops)
**Demand:** 🔥🔥🔥 (Now in v1.0, was #1 requested feature for months)
- **Source:** [obsidian-clipper v1.0 release](https://www.reddit.com/r/ObsidianMD/comments/1r7a96k/obsidian_web_clipper_10_now_with_logic/)
- **Source:** [obsidian-clipper Roadmap](https://github.com/obsidianmd/obsidian-clipper) — "Template logic (if/for)"
- **What users want:** `{% if %}` conditionals, `{% for %}` loops, variable assignment in templates
- **Status:** ✅ Implemented in v1.0 (March 2026). Shows this was THE most requested feature.

### 8. Auto-Detect Content Type / Smart Templates
**Demand:** 🔥🔥🔥 (Forum, Reddit)
- **Source:** [Obsidian Forum](https://forum.obsidian.md/t/web-clipper-add-meta-variable-based-template-matching/91950) — "Add meta variable-based template matching"
- **Source:** Reddit — Users want different templates for articles, recipes, tweets, LinkedIn posts
- **What users want:** Auto-apply templates based on URL patterns, meta tags, schema.org data, content type detection
- **Current:** URL regex triggers only. Users want meta tag and content-type triggers too.

### 9. Dataview-Compatible Properties (YAML Frontmatter)
**Demand:** 🔥🔥🔥 (Multiple issues, forum posts)
- **Source:** [obsidian-omnivore #15](https://github.com/mvavassori/obsidian-web-clipper/issues/15) — "Add URL and timestamp as note properties for Dataview compatibility"
- **What users want:** Rich frontmatter with: `url`, `date`, `author`, `tags`, `source`, `readingTime`, `language`, `contentType`, `status`
- **Example query users want:** `table date, url from "bookmarks" where url and date sort date asc`
- **Key insight:** Power users build entire systems on top of clipped content metadata

### 10. Background Processing / Async Clipping
**Demand:** 🔥🔥🔥 (Recent, LLM-related)
- **Source:** [obsidian-clipper #720](https://github.com/obsidianmd/obsidian-clipper/issues/720) — "Background processing for Interpreter LLM requests"
- **What users want:** Don't keep popup open during LLM processing. Process in background, notify when done.
- **Use case:** AI summarization, auto-tagging, content extraction via LLM

### 11. Notification on Clip Success/Failure
**Demand:** 🔥🔥🔥 (Forum, GitHub #576)
- **Source:** [Obsidian Forum](https://forum.obsidian.md/t/obsidian-web-clipper-notification-support/96624) — "Obsidian web clipper notification support"
- **Source:** [obsidian-clipper #576](https://github.com/obsidianmd/obsidian-clipper/issues/576) — "Notification when Interpreter is done"
- **What users want:** Desktop notification: "Note 'test' was successfully saved at 'Clippings'"
- **Why:** Users clip and move on; need confirmation without checking

### 12. Batch Clip All Open Tabs
**Demand:** 🔥🔥🔥 (MarkDownload #15, recurring request)
- **Source:** [MarkDownload #15](https://github.com/deathau/markdownload/issues/15) — "Save all tabs in window"
- **Source:** Obsidian Forum — "batch MarkDownload all open tabs in current Firefox window"
- **What users want:** Open 10 research tabs → one click → all saved to vault
- **Use case:** Research sessions, course material collection

### 13. Code Block Preservation with Syntax Highlighting
**Demand:** 🔥🔥🔥 (MarkDownload #371, #395)
- **Source:** [MarkDownload #371](https://github.com/deathau/markdownload/issues/371) — "Extension struggles to extract code blocks to markdown format"
- **Source:** [MarkDownload #395](https://github.com/deathau/markdownload/issues/395) — "MarkDownload leaves a lot of in extracted code samples"
- **What users want:** Proper fenced code blocks with language detection, preserved indentation, no HTML artifacts

### 14. LaTeX / Math Preservation
**Demand:** 🔥🔥🔥 (MarkDownload #373, #377)
- **Source:** [MarkDownload #373](https://github.com/deathau/markdownload/issues/373) — "need a way to extract latex already rendered by mathjax"
- **Source:** [MarkDownload #377](https://github.com/deathau/markdownload/issues/377) — "Uncaught ReferenceError: MathJax is not defined"
- **What users want:** Convert MathJax/KaTeX rendered math back to LaTeX syntax (`$...$`, `$$...$$`)
- **Use case:** Academic papers, math blogs, StackExchange

### 15. Social Media / Platform-Specific Extraction
**Demand:** 🔥🔥🔥 (Multiple bugs, workarounds)
- **Source:** [obsidian-clipper #676](https://github.com/obsidianmd/obsidian-clipper/issues/676) — "Issues/inconsistencies with X / Twitter Articles"
- **Source:** [obsidian-clipper #332](https://github.com/obsidianmd/obsidian-clipper/issues/332) — "x.com fails to get relevant data for selected tweet"
- **Source:** Reddit — "Do you have any solutions in mind for LinkedIn scraping?"
- **What users want:** Clean extraction from Twitter/X, LinkedIn, Reddit, YouTube (with transcripts), ChatGPT conversations
- **Current state:** Defuddle (content extractor) handles articles well but struggles with social platforms

---

## 2. Pain Points Analysis

### Most Frustrating Issues (by frequency of complaints)

| Pain Point | Frequency | Severity | Sources |
|------------|-----------|----------|---------|
| **Duplicate clips** | Very High | Critical | All tools, all platforms |
| **Manual folder path typing** | Very High | High | Reddit, Forum, GitHub |
| **Mobile clipping friction** | High | High | Reddit, Forum |
| **Image handling (broken URLs)** | High | High | MarkDownload, Forum |
| **No offline images** | High | Medium | Roadmap item |
| **Social media extraction fails** | High | Medium | GitHub bugs |
| **PDF content not extracted** | Medium | High | GitHub #646 |
| **No notification on save** | Medium | Medium | Forum, GitHub |
| **Template complexity** | Medium | Medium | Forum, Reddit |
| **Math/LaTeX lost** | Medium | Medium | GitHub issues |
| **Code blocks mangled** | Medium | Medium | GitHub issues |
| **Sync conflicts (Omnivore)** | Medium | High | GitHub issues |
| **No batch operations** | Low-Medium | Medium | Forum, GitHub |
| **iOS Safari bugs** | Low-Medium | High | GitHub #588 |
| **Regex double-escaping** | Low | Annoying | GitHub #717 |

### User Quotes (Direct from sources)

> "It's so laborious to have to either type the folder path, or save to obsidian and then move the file for every single web clipping."
> — Reddit r/ObsidianMD

> "I mean it works, but hell that's so much friction" (about iOS clipping workflow)
> — Reddit r/ObsidianMD

> "Is it that bad to ask for this feature? I just want to not end up with duplicate web clippings. That's all."
> — GitHub #112 comment

> "The highlighting lags and floats over the screen sometimes"
> — Reddit r/ObsidianMD

> "I'd love it if you could do simple sums and multiplications in markdown tables"
> — MarkDownload user (shows desire for richer content)

---

## 3. Feature Categories

### A. Workflow & UX
| Feature | Demand | Complexity |
|---------|--------|------------|
| Duplicate detection | 🔥🔥🔥🔥🔥 | Medium |
| Quick save to default location | 🔥🔥🔥🔥 | Low |
| Batch clip all tabs | 🔥🔥🔥 | Medium |
| Mobile share sheet integration | 🔥🔥🔥🔥 | High |
| Notification on success/failure | 🔥🔥🔥 | Low |
| Incremental clipping (append) | 🔥🔥🔥🔥 | Medium |
| Background async processing | 🔥🔥🔥 | Medium |

### B. Content Quality
| Feature | Demand | Complexity |
|---------|--------|------------|
| Save images locally | 🔥🔥🔥🔥 | Medium |
| PDF extraction | 🔥🔥🔥🔥🔥 | High |
| Code block preservation | 🔥🔥🔥 | Medium |
| LaTeX/Math preservation | 🔥🔥🔥 | High |
| Social media extraction | 🔥🔥🔥 | High |
| Footnote handling | 🔥🔥 | Medium |
| Table preservation (GFM) | 🔥🔥 | Low-Medium |
| Video embedding (YouTube) | 🔥🔥 | Low |
| Tweet embedding | 🔥🔥 | Medium |

### C. Obsidian Integration
| Feature | Demand | Complexity |
|---------|--------|------------|
| Dataview-compatible frontmatter | 🔥🔥🔥 | Low |
| Auto-detect vault path | 🔥🔥🔥 | Low |
| Template triggers (URL, meta, content-type) | 🔥🔥🔥 | Medium |
| Obsidian Local REST API integration | 🔥🔥 | Medium |
| Obsidian URI protocol support | 🔥🔥 | Low |
| Git integration awareness | 🔥🔥 | Low |
| Canvas file generation | 🔥 | Medium |
| MOC auto-generation | 🔥 | Medium |

### D. AI/Smart Features
| Feature | Demand | Complexity |
|---------|--------|------------|
| Auto-tagging | 🔥🔥🔥 | Medium |
| Auto-summarization | 🔥🔥🔥 | Medium |
| Duplicate detection (semantic) | 🔥🔥🔥 | High |
| Key quotes extraction | 🔥🔥 | Medium |
| Action items extraction | 🔥🔥 | Medium |
| Reading time estimation | 🔥🔥 | Low |
| Language detection | 🔥🔥 | Low |
| Difficulty level detection | 🔥 | Medium |
| Author/topic clustering | 🔥 | High |

### E. Advanced Obsidian Features
| Feature | Demand | Complexity |
|---------|--------|------------|
| Rich YAML properties | 🔥🔥🔥 | Low |
| Templater-compatible output | 🔥🔥 | Low |
| Dataview query-ready metadata | 🔥🔥🔥 | Low |
| Backlink-friendly wiki links | 🔥🔥 | Medium |
| Graph view optimization | 🔥🔥 | Medium |
| Tags vs folder flexibility | 🔥🔥 | Low |
| Excalidraw diagram conversion | 🔥 | High |

---

## 4. Quick Wins (Low Effort, High Value)

### Tier 1 — Implement First
| Feature | Why | Effort |
|---------|-----|--------|
| **Rich YAML frontmatter** | Power users demand it for Dataview. Easy to generate. | 1-2 days |
| **Duplicate URL detection** | #1 complaint across all tools. Simple URL hash check. | 1-2 days |
| **Auto-detect vault** | Scan for `.obsidian` folder, check `OBSIDIAN_VAULT` env var. | 1 day |
| **Reading time estimation** | Simple word count / 200 WPM. Users love this metadata. | 2 hours |
| **Language detection** | Use `whatlang` crate. One dependency, high value. | 2 hours |
| **Notification on save** | Desktop notification. One system call. | 2 hours |
| **Obsidian URI support** | Open saved note via `obsidian://open?vault=X&file=Y`. | 2 hours |

### Tier 2 — Next Sprint
| Feature | Why | Effort |
|---------|-----|--------|
| **Quick save mode** | Pre-configured vault + folder. One command. | 1 day |
| **Image download to vault** | Download images, save to assets/, use wiki links. | 2-3 days |
| **Template system** | Configurable output templates (like MarkDownload). | 3-5 days |
| **GFM table support** | Proper markdown tables from HTML tables. | 1-2 days |
| **Footnote handling** | Convert HTML footnotes to markdown footnotes. | 1-2 days |
| **YouTube embed conversion** | Convert YouTube URLs to `![[youtube|url]]` or embed syntax. | 1 day |

---

## 5. Differentiators (Features NO Competitor Has)

### 1. **CLI-First Architecture** (Unique)
All competitors are browser extensions. A Rust CLI can:
- Run headless on servers for batch processing
- Integrate into CI/CD pipelines
- Process sitemaps and entire sites
- Run as a cron job for scheduled clipping
- **No competitor offers this.**

### 2. **WAF/CAPTCHA Bypass** (Unique for Obsidian tools)
Your scraper already has WAF detection (19 signatures). Browser extensions can't bypass these.
- **Competitors:** All fail on Cloudflare-protected pages
- **Your advantage:** Can handle rate limiting, UA rotation, retry logic

### 3. **Semantic Duplicate Detection** (Unique)
Not just URL matching — use embeddings to detect semantically similar content.
- **Competitors:** Only do exact URL matching (and even that is inconsistent)
- **Your advantage:** Already has AI/ONNX infrastructure

### 4. **Streaming + Constant RAM** (Unique)
Target ~8KB constant RAM for large pages. Browser extensions load entire page in memory.
- **Competitors:** All load full page DOM
- **Your advantage:** Can handle massive pages without crashing

### 5. **Sitemap-to-Vault Pipeline** (Unique)
Process entire sitemaps → batch clip → organize in vault structure.
- **Competitors:** One page at a time, manually triggered
- **Your advantage:** Already has sitemap parsing

### 6. **Auto-Generated MOCs** (Unique)
After clipping multiple pages on a topic, auto-generate a Map of Content index page.
- **Competitors:** None do this
- **Your advantage:** Can analyze content relationships and create structure

### 7. **Git-Aware Sync** (Unique)
Detect if vault uses Git, create meaningful commits for each clip.
- **Competitors:** None integrate with Git
- **Your advantage:** Can track changes, enable collaboration

### 8. **Content Type Auto-Detection** (Partially Unique)
Detect articles, recipes, products, papers, tweets, and apply appropriate templates automatically.
- **Competitors:** Only URL regex triggers
- **Your advantage:** Can use schema.org, meta tags, content analysis

---

## 6. Recommended P2 Roadmap

### Phase 1: Foundation (Weeks 1-3)
**Goal:** Make it work well for basic Obsidian users

| Priority | Feature | Justification |
|----------|---------|---------------|
| P2-1 | Rich YAML frontmatter | Most requested metadata feature. Enables Dataview queries. |
| P2-2 | Duplicate URL detection | #1 user complaint. Simple to implement. |
| P2-3 | Auto-detect vault | Removes friction. Check `.obsidian` + env var. |
| P2-4 | Quick save mode | One-command save to pre-configured location. |
| P2-5 | Image download to vault | On official Obsidian roadmap. High demand. |

### Phase 2: Content Quality (Weeks 4-6)
**Goal:** Make clipped content look great in Obsidian

| Priority | Feature | Justification |
|----------|---------|---------------|
| P2-6 | GFM table support | Tables are common, current tools mangle them. |
| P2-7 | Footnote handling | Academic users need this. |
| P2-8 | Code block preservation | Developers clip docs constantly. |
| P2-9 | YouTube/video embed conversion | Easy win, high perceived value. |
| P2-10 | Reading time + language metadata | Easy metadata additions users love. |

### Phase 3: Smart Features (Weeks 7-9)
**Goal:** AI-powered features that competitors can't match

| Priority | Feature | Justification |
|----------|---------|---------------|
| P2-11 | Auto-tagging based on content | Uses existing AI infrastructure. High value. |
| P2-12 | Auto-summarization for long articles | Uses existing AI infrastructure. |
| P2-13 | Semantic duplicate detection | Unique differentiator. Uses embeddings. |
| P2-14 | Key quotes extraction | Researchers love this. |
| P2-15 | Content type auto-detection | Better than URL regex triggers. |

### Phase 4: Advanced Integration (Weeks 10-12)
**Goal:** Deep Obsidian ecosystem integration

| Priority | Feature | Justification |
|----------|---------|---------------|
| P2-16 | Obsidian URI support | Open notes directly in Obsidian. |
| P2-17 | Auto-generated MOCs | Unique feature. Power users will love it. |
| P2-18 | Git-aware vault support | Niche but passionate user base. |
| P2-19 | Template system | Configurable output formats. |
| P2-20 | Sitemap batch processing | Unique differentiator. |

---

## Appendix: Source Links

### GitHub Issues
- [obsidian-clipper #112](https://github.com Obsidianmd/obsidian-clipper/issues/112) — Duplicate detection (10 reactions)
- [obsidian-clipper #646](https://github.com/obsidianmd/obsidian-clipper/issues/646) — PDF clipping
- [obsidian-clipper #720](https://github.com/obsidianmd/obsidian-clipper/issues/720) — Background LLM processing
- [obsidian-clipper #714](https://github.com/obsidianmd/obsidian-clipper/issues/714) — Multiline properties
- [obsidian-clipper #717](https://github.com/obsidianmd/obsidian-clipper/issues/717) — Regex literal syntax
- [obsidian-clipper #676](https://github.com/obsidianmd/obsidian-clipper/issues/676) — X/Twitter articles
- [obsidian-clipper #332](https://github.com/obsidianmd/obsidian-clipper/issues/332) — X/Twitter replies
- [obsidian-clipper #588](https://github.com/obsidianmd/obsidian-clipper/issues/588) — iOS Safari bugs
- [obsidian-clipper Roadmap](https://github.com/obsidianmd/obsidian-clipper) — Official roadmap
- [MarkDownload #21](https://github.com/deathau/markdownload/issues/21) — Quick save
- [MarkDownload #41](https://github.com/deathau/markdownload/issues/41) — Default folder
- [MarkDownload #371](https://github.com/deathau/markdownload/issues/371) — Code blocks
- [MarkDownload #373](https://github.com/deathau/markdownload/issues/373) — LaTeX/MathJax
- [MarkDownload #342](https://github.com/deathau/markdownload/discussions/342) — Future of MarkDownload
- [obsidian-omnivore #15](https://github.com/mvavassori/obsidian-web-clipper/issues/15) — Dataview properties
- [obsidian-omnivore #179](https://github.com/omnivore-app/obsidian-omnivore/issues/179) — Sync issues
- [obsidian-omnivore #33](https://github.com/omnivore-app/obsidian-omnivore/issues/33) — Deduplication

### Obsidian Forum
- [Incremental clipping](https://forum.obsidian.md/t/web-clipper-append-new-highlights-to-existing-notes-incremental-clipping/109677)
- [Meta variable template matching](https://forum.obsidian.md/t/web-clipper-add-meta-variable-based-template-matching/91950)
- [Notification support](https://forum.obsidian.md/t/obsidian-web-clipper-notification-support/96624)
- [Auto-add URL to web viewer](https://forum.obsidian.md/t/web-viewer-auto-add-original-url-of-the-saved-webpage/95759)
- [MarkDownload thread](https://forum.obsidian.md/t/markdownload-markdown-web-clipper/173)

### Reddit
- [iOS workflow friction](https://www.reddit.com/r/ObsidianMD/comments/1r3la2x/how_do_you_guys_use_webclipper_on_ios/)
- [Web Clipper 1.0 discussion](https://www.reddit.com/r/ObsidianMD/comments/1r7a96k/obsidian_web_clipper_10_now_with_logic/)
- [Highlighting issues](https://www.reddit.com/r/ObsidianMD/comments/1k6g03y/issues_with_obsidian_web_clipper_edge/)

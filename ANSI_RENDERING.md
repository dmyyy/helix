# ANSI rendering in Helix

## Goal

Add an optional mode for pager-like buffers so ANSI-colored output opens in Helix looking close to the terminal output instead of as plain text.

This is mainly useful for:

- scrollback opened via stdin (`Editor::new_file_from_stdin` in `helix-view/src/editor.rs`)
- `:insert-output` / `:append-output`
- `man`-style buffers

One hard constraint: if the terminal/editor handoff has already stripped ANSI escape sequences, Helix cannot reconstruct the lost colors. This feature only works when the buffer still contains the raw escape sequences.

## What `kak-ansi-rs` is doing

`kak-ansi-rs` is a good model because it is **not** a full terminal emulator. It is a pager filter.

From `src/filter.rs`, `src/face.rs`, and `rc/ansi.kak`:

- it parses SGR color/style sequences
- strips escape sequences from the visible text
- emits style ranges separately
- handles man/nroff overstrikes (`X\bX`, `_\bX`)
- deletes carriage returns
- swallows OSC and non-SGR CSI sequences
- translates DEC line-drawing mode
- auto-enables for stdin/man buffers

That scope is a good fit for Helix scrollback. It is enough for logs, test output, CI output, `man`, compiler errors, etc. It is **not** enough for replaying full-screen TUIs like `htop`, `vim`, or anything that depends on cursor motion / screen clearing.

## Helix constraints today

Helix already has most of the rendering primitives needed, but not in the right shape.

### What Helix already has

- `helix-view::graphics::Style` supports:
  - foreground/background colors
  - indexed ANSI colors
  - RGB colors
  - bold/dim/italic/blink/reverse/crossed-out
  - underline style/color
- the TUI backend already emits indexed and RGB ANSI colors (`helix-view/src/graphics.rs`, `helix-tui/src/backend/crossterm.rs`)
- the document renderer already merges syntax styles with overlay styles (`helix-term/src/ui/document.rs`)
- Helix already has view-time overlay ranges via `OverlayHighlights` (`helix-core/src/syntax.rs`)

### What is missing

1. `OverlayHighlights` only carries `Highlight`, not arbitrary `Style`.
2. `Theme::rgb_highlight` only encodes an RGB **foreground** (`helix-view/src/theme.rs`), so it cannot represent ANSI background colors or modifier-heavy faces.
3. `TextAnnotations::Overlay` is a one-grapheme replacement, not a “hide this whole escape range” mechanism (`helix-core/src/text_annotations.rs`).
4. The current render pipeline assumes the document text is the text to display; it does not have a native “parse ANSI and conceal escape runs” layer.

Because of that, a pure render-time mode over the raw buffer is possible, but substantially larger than it sounds.

## Recommended design

The lowest-risk design is a **one-time ANSI-to-plain-text transform plus cached style spans**, modeled after `kak-ansi-rs`.

### Why this is the right first implementation

It avoids building a mini terminal emulator into the render loop.

Instead of reparsing ANSI every frame, Helix would:

1. parse the buffer once
2. produce plain visible text
3. record styled spans over that visible text
4. render those spans as overlay styles

That matches the actual scrollback use case: open output, read it, maybe search it, maybe copy it.

## Proposed feature shape

### User-facing behavior

Recommended surface area:

- `:ansi-render` — parse current buffer, replace it with visible text, attach ANSI style spans
- optional config to auto-run this for stdin buffers, default off
  - e.g. `editor.ansi-render-stdin = true`
- maybe later: auto-run for `man` buffers too

I would **not** start with a reversible toggle. A true toggle requires keeping the original raw buffer around or adding a much larger render-time conceal layer.

For scrollback, an explicit transform is enough.

## Parser scope

Lift the parser behavior almost directly from `kak-ansi-rs`.

### Supported

- SGR reset / attributes / colors
  - 16-color
  - 256-color
  - truecolor
- bold, dim, italic, underline, blink, reverse, strikethrough
- `\r` deletion
- OSC swallowing
- non-SGR CSI swallowing
- DEC line drawing
- SO/SI charset switching
- nroff/man overstrike handling

### Explicit non-goals

- cursor movement
- erase-in-display / erase-in-line semantics
- alternate screen replay
- terminal stateful layout emulation

If we try to support full terminal replay, this stops being a document-highlighting feature and becomes a terminal emulator project.

## Internal data model

Add a new cached document payload, something like:

```rust
struct AnsiStyledSpan {
    range: std::ops::Range<usize>, // char range in transformed text
    style: Style,
}

struct AnsiRenderState {
    spans: Vec<AnsiStyledSpan>,
}
```

Store it on `Document` in `helix-view/src/document.rs`.

Important: the ranges should be in **char indices**, because Helix’s view/highlight pipeline is char-oriented.

## Rendering changes

This is the main engine change.

### Needed addition

Add a direct-`Style` overlay path alongside `OverlayHighlights`.

Today the render path is roughly:

- syntax highlight -> `Style`
- overlay highlights -> `Highlight` -> theme lookup -> `Style`
- patch styles together

For ANSI we need:

- syntax highlight -> `Style`
- regular overlay highlights -> `Style`
- ANSI style spans -> `Style`
- patch all three together

That means adding a new style-span highlighter/cursor in the same area as `OverlayHighlighter` usage in `helix-term/src/ui/document.rs` and `helix-term/src/ui/editor.rs`.

I would not try to force ANSI through `Theme::rgb_highlight` because that loses background/reset fidelity and makes modifiers awkward.

## Text transformation path

When `:ansi-render` runs:

1. read the current `Rope` as text
2. parse it with an in-process ANSI filter
3. build:
   - transformed visible text
   - styled spans over transformed text
4. replace the document contents with the transformed text
5. attach the cached ANSI spans to the document
6. request redraw

For the scrollback case, this should happen once after `new_file_from_stdin` fills the scratch buffer.

## Edit semantics

Keep this simple.

If the user edits an ANSI-rendered buffer, clear the cached ANSI spans on first edit and keep the visible text.

Why:

- reparsing incrementally is extra complexity for a weak use case
- keeping raw ANSI + rendered text in sync is expensive and confusing
- scrollback/man buffers are mostly read-only anyway

So the mental model is:

- render once
- view/search/copy normally
- editing drops ANSI metadata

## Auto-enable path for scrollback

The best hook is the stdin path already used by Helix:

- `helix-term/src/application.rs` chooses `new_file_from_stdin()`
- `helix-view/src/editor.rs` populates the scratch buffer

An opt-in config can run ANSI rendering immediately after that load if the buffer contains `ESC` sequences.

That gives the user-visible win without affecting normal file opens.

## Fidelity notes

### Good news

Helix can already emit terminal indexed colors and RGB colors, so matching terminal palettes is realistic for most ANSI runs.

### Caveat

“default foreground/background” is trickier.

If ANSI-rendered text should match the terminal’s default palette exactly, the ANSI span styles should be allowed to use `Color::Reset` / explicit bg+fg resets, not just “no overlay”. Otherwise Helix will fall back to `ui.text`, which may differ from the terminal default colors.

That is another reason to use direct `Style` spans instead of theme `Highlight`s.

## Suggested file-level implementation plan

### 1. Parser

Add a small parser module, ideally in `helix-core` so it is UI-independent.

Suggested output:

```rust
struct AnsiRenderOutput {
    text: Rope,
    spans: Vec<AnsiStyledSpan>,
}
```

Base it closely on the `kak-ansi-rs` state machine.

### 2. Document state

Add optional ANSI render state to `Document` in `helix-view/src/document.rs`.

### 3. Commands/config

Add:

- `:ansi-render`
- optional `editor.ansi-render-stdin`

Command wiring belongs in `helix-term/src/commands/typed.rs`.

### 4. Render pipeline

Extend `helix-term/src/ui/editor.rs` and `helix-term/src/ui/document.rs` with a direct-style span overlay path.

### 5. Invalidation

On document edit, clear ANSI render state.

## Test plan

### Parser tests

Port the useful cases from `kak-ansi-rs`:

- color transitions
- reset behavior
- 256 color / truecolor
- overstrikes
- CR deletion
- OSC swallowing
- DEC line drawing
- invalid UTF-8 handling if the parser accepts bytes directly

### Rendering tests

Add focused render tests for:

- foreground + background together
- reverse video
- reset to terminal default colors
- multiline spans
- span alignment after stripped escape sequences

### Integration tests

At least one stdin-buffer test should verify:

- raw ANSI input enters Helix
- auto-render (if enabled) transforms text
- visible buffer text no longer contains escape sequences
- styles are present on the expected ranges

## What I would avoid

- reparsing ANSI in the hot render loop
- encoding ANSI styles as theme scopes
- extending syntax highlighting to pretend ANSI is a language
- trying to support full-screen TUI replay
- building a reversible toggle in v1

## Bottom line

A practical implementation is very doable, but the right shape is:

- **pager-mode ANSI parser**
- **destructive text normalization**
- **cached direct `Style` spans**
- **optional auto-enable for stdin buffers**

That solves the scrollback readability problem without turning Helix’s renderer into a terminal emulator.
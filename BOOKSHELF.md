Do one phase at a time. After each phase run the tests and show me the result before continuing. Do not combine all phases into one large rewrite


# PIC — Optional iPhoto Bookshelf Albums View

## Goal

Add an **optional bookshelf presentation for Albums Home**.

The existing/default Albums Home must remain intact.

Behaviour:

```text
Standard theme
    → always current/default Albums Home

iPhone theme + Bookshelf OFF
    → current/default Albums Home

iPhone theme + Bookshelf ON
    → new iPhoto bookshelf Albums Home
```

The bookshelf is only a **presentation option**. Do not change album database structure, album membership, photo navigation, sidebar behaviour, or individual album views.

The current database already supplies album `id`, `name`, and `photo_count`; no schema change is required. 

Assets already exist in:

```text
images/bookshelf.png
images/white-album.png
images/pink-album.png
```

Any future:

```text
images/*-album.png
```

must automatically become available as an album style.

---

# PHASE 1 — Add the setting only

**Goal:** Add the persisted Bookshelf ON/OFF option without changing Albums Home yet.

Modify only the minimum necessary files.

Use a setting key:

```rust
"iphone-bookshelf-albums"
```

Default:

```text
false
```

Add the option under:

```text
Settings
→ Themes
→ iPhone
```

UI:

```text
iPhone

Use iPhoto bookshelf for Albums    [ OFF ]
```

Persist it using the existing database settings mechanism:

```rust
db::setting(...)
db::set_setting(...)
```

The existing settings table already supports arbitrary persisted key/value settings. 

The app already persists the appearance theme with a settings key, so follow that existing pattern rather than inventing another configuration system. 

### Phase 1 success test

Run:

```bash
cargo fmt --check
cargo test
cargo check
cargo run
```

Verify:

```text
Settings → Themes shows bookshelf toggle
toggle survives app restart
Albums Home still looks exactly as it did before
Standard theme is unchanged
```

**STOP after Phase 1. Do not begin bookshelf rendering until this works.**

Suggested commit:

```bash
git add src/
git commit -m "feat: add iPhone bookshelf albums setting"
```

---

# PHASE 2 — Discover the album PNG skins

**Goal:** Teach Albums Home how to find available album covers, but do not change the UI yet.

Work primarily in:

```text
src/albums_view.rs
```

Discover files from:

```text
images/
```

Rules:

```text
white-album.png       YES
pink-album.png        YES
blue-album.png        YES
leather-album.png     YES

bookshelf.png         NO
album.png             NO
cover.png             NO
white-album.jpg       NO
```

Use a small helper such as:

```rust
fn album_frame_paths() -> Vec<PathBuf>
```

Only files matching:

```text
*-album.png
```

are included.

Sort the filenames before using them.

Do not hard-code:

```rust
["white-album.png", "pink-album.png"]
```

Adding this later:

```text
images/green-album.png
```

must require **zero Rust changes**.

### Stable random selection

Albums should look randomly coloured, but must not change colour every refresh.

Use a deterministic selection based on album identity:

```rust
fn frame_index(album: &Album, frame_count: usize) -> Option<usize>
```

Base it on something stable such as:

```text
album.id + album.name
```

Requirements:

```text
same album + same skins → same skin
0 skins → None
1 skin → always skin 0
multiple skins → stable random-looking distribution
```

Do not use a random generator every time Albums Home refreshes.

### Phase 2 tests

Add simple unit tests for:

```text
*-album.png matching
zero skins
one skin
stable selection
selection stays within bounds
```

Run:

```bash
cargo fmt --check
cargo test
cargo check
```

No visible Albums Home changes yet.

Suggested commit:

```bash
git add src/albums_view.rs
git commit -m "feat: discover album cover skins"
```

---

# PHASE 3 — Build the bookshelf renderer separately

**Goal:** Add the new renderer without deleting or rewriting the current one.

This is important:

**KEEP THE EXISTING DEFAULT ALBUM VIEW.**

Do not throw away the current `albums_view.rs` behaviour.

Split the rendering logically into:

```rust
populate_default(...)
populate_bookshelf(...)
```

or equivalent.

Then have the main Albums Home population decide which one to use.

Conceptually:

```rust
if theme == "iphone" && bookshelf_enabled {
    populate_bookshelf(...);
} else {
    populate_default(...);
}
```

Do not duplicate database queries unnecessarily.

Do not make two separate album systems.

The current album view already retrieves the album's photos and uses cached thumbnails. Preserve that logic.

PIC's existing thumbnail cache is already fingerprinted and should continue to supply album cover previews instead of decoding full originals. 

### Bookshelf structure

Use:

```text
images/bookshelf.png
```

as the background of Albums Home.

Keep the current responsive album layout if possible.

A reasonable GTK structure is:

```text
ScrolledWindow
    Overlay
        bookshelf background
        Albums content
```

Do not modify the normal photo grid.

Do not modify an opened individual album.

Do not modify sidebar navigation.

### Phase 3 initial appearance

For this phase it is enough to get:

```text
bookshelf visible
album cards visible over bookshelf
album title below
photo count below
clicking album still works
```

Do **not yet** put the photo into `white-album.png`.

First make the new view switch correctly.

### Phase 3 verification

Check:

```text
Standard → default Albums Home

iPhone + Bookshelf OFF
→ default Albums Home

iPhone + Bookshelf ON
→ bookshelf Albums Home

turn option OFF
→ original Albums Home returns
```

Switching the option must not lose or modify albums.

Suggested commit:

```bash
git add src/
git commit -m "feat: add optional bookshelf albums view"
```

---

# PHASE 4 — Add the album PNG overlay

**Goal:** Make each album look like a physical iPhoto album.

Each album on the bookshelf should become:

```text
         selected *-album.png
        ┌───────────────────┐
        │   ┌───────────┐   │
        │   │ COVER     │   │
        │   │ PHOTO     │   │
        │   └───────────┘   │
        └───────────────────┘

             Holiday
            83 photos
```

Layering:

```text
BOTTOM
    album cover photograph

TOP
    randomly selected *-album.png
```

The PNG is the physical album/frame.

The photograph must fit into the rectangular opening.

Do not flatten these into a new generated image.

Use GTK widgets.

### Important

Preserve the existing cover-photo selection logic.

Do not introduce another decoder.

Preserve support for:

```text
JPEG
RAW
rotation
edited thumbnails
missing thumbnails
```

The current `PhotoObject` already tracks cached thumbnail availability and paths. 

### Geometry

Inspect:

```text
images/white-album.png
```

Measure the photo opening.

Use one common geometry for all:

```text
*-album.png
```

because these files are skins of the same album design.

Prefer normalized ratios:

```rust
const PHOTO_X_RATIO: f64 = ...;
const PHOTO_Y_RATIO: f64 = ...;
const PHOTO_WIDTH_RATIO: f64 = ...;
const PHOTO_HEIGHT_RATIO: f64 = ...;
```

Do not distort the album PNG.

Use:

```rust
gtk::ContentFit::Cover
```

for the actual cover photo so it fills the opening.

If there is no album cover image:

```text
keep album PNG visible
show subtle placeholder inside photo opening
```

### Album labels

Keep:

```text
Holiday
83 photos
```

below the album.

Keep existing singular/plural behaviour:

```text
1 photo
2 photos
```

Do not write the title onto the album yet.

### Phase 4 verification

Test:

```text
portrait cover
landscape cover
RAW cover
edited/rotated cover
empty album
1-photo album
long album title
multiple albums
```

Make sure several albums use different available `*-album.png` skins.

Suggested commit:

```bash
git add src/albums_view.rs
git commit -m "feat: add album cover overlays to bookshelf"
```

---

# PHASE 5 — Polish only after functionality works

Do not start this phase until Phases 1–4 work.

Adjust only:

```text
album size
album spacing
shelf alignment
text position
background scaling
hover effect
```

Target appearance:

```text
 [album]       [album]       [album]
 Holiday       Drone         Family
 83 photos     241 photos    55 photos
──────────────── wooden shelf ───────────────

 [album]       [album]       [album]
 Sports        Cape Town     Portfolio
 29 photos     104 photos    18 photos
──────────────── wooden shelf ───────────────
```

The album objects should look as if they are physically resting on the shelves.

Avoid:

```text
large GTK card backgrounds
thick borders
large padding
photo-grid style selection boxes
```

The PNG already provides the visual frame.

Keep hover feedback subtle.

---

# PHASE 6 — Final verification

Run:

```bash
cargo fmt --check
cargo test
cargo check
git diff --check
```

Then:

```bash
cargo run
```

Verify all three modes:

```text
STANDARD
Bookshelf setting irrelevant
→ default Albums Home

IPHONE + BOOKSHELF OFF
→ default Albums Home

IPHONE + BOOKSHELF ON
→ bookshelf Albums Home
```

Also verify:

```text
toggle survives restart
albums still open normally
album count updates
creating an album works
adding photos works
empty album works
returning to Albums Home works
normal photo grid unchanged
lightbox unchanged
editing unchanged
collage unchanged
```

Then test dynamic skins:

1. Run with:

```text
white-album.png
pink-album.png
```

2. Add:

```text
images/test-album.png
```

3. Restart PIC.

`test-album.png` must automatically become part of the skin pool.

No Rust modification should be necessary.

---

# Important restrictions

Do not:

```text
rewrite Albums
change the database schema
change album membership
change SidebarFilter::Album
change individual album photo view
change normal photo grid
change collage
change editor
change lightbox
hard-code pink/white skins
use /home/peet paths
use /mnt/data paths
decode full-resolution originals for album covers
change album colour every refresh
remove the current/default Albums Home
```

This feature is:

```text
existing Albums Home
        +
optional bookshelf renderer
        +
iPhone-theme toggle
```

Nothing more.

---

## Final desired architecture

Keep it conceptually this simple:

```rust
Albums Home
    |
    +-- Standard theme
    |       └── Default renderer
    |
    +-- iPhone theme
            |
            +-- Bookshelf OFF
            |       └── Default renderer
            |
            +-- Bookshelf ON
                    └── Bookshelf renderer
                            |
                            +-- bookshelf.png
                            +-- cached album cover photo
                            +-- stable random *-album.png
                            +-- album name
                            └-- photo count
```




# Catbot desktop pet, 3D

A slender robot cat, modelled, rigged, animated and rendered by one Blender script. The app
shows it as pre-rendered sprite sheets; nothing 3D runs in the pet window.

Status: second pass. The first preview rendered cleanly on Blender 5.3 (Eevee) and the tint
recipe, visor and LEDs held up; this pass reshaped the body (one continuous shell, tapered legs,
haunches, whip tail) and reworked the sleep and sitting poses. The new shapes have only been seen
in the rough software renderer (`soft_preview.py`), so run the preview again before the full run.

## Files

| File | What it is |
| --- | --- |
| `build_pet.py` | Builds everything and renders. Runs inside Blender 4.0 or newer. |
| `render.json` | What to render: `{"mode": "preview"}` or `{"mode": "full"}`. |
| `soft_preview.py` | Flat-shaded software render of the same poses (numpy + Pillow, no Blender). For checking silhouettes and framing only. |
| `renders/` | Output (made by the script). |

## Running

```
blender -b --factory-startup --python-exit-code 1 -P art/pet3d/build_pet.py -- \
    --config art/pet3d/render.json --out art/pet3d/renders
```

`render.json` keys (all but `mode` optional):

| Key | Default | Meaning |
| --- | --- | --- |
| `mode` | `"preview"` | `"preview"`: stills and a contact sheet. `"full"`: sprite sheets and `manifest.json`. `"icon"`: the app icon. `"marks"`: the in-app bot marks. |
| `icon_tint` | `[1, 164, 146]` | Icon mode: the shell colour, RGB 0..255. The default is Catbot's teal `#01A492`. |
| `only` | all | Full mode: a list of animation names to render. The manifest is not rewritten when this is set. |
| `passes` | `["base","mask","gloss"]` | Full mode: which passes to render. |
| `scale` | `2` | Frame size as a multiple of 96 x 80. |
| `samples` | `24` | Eevee samples per render. |
| `engine` | `"auto"` | `"auto"`, `"eevee"` or `"cycles"`. Auto tries Eevee under both of its names, then Cycles on the CPU. |
| `shell_level` | `0.80` | How bright the lit shell should come out (0..1, display value). The script measures a test render and scales the lights to hit it. |
| `save_blend` | `true` | Also save `pet.blend` with one Action per animation (`pet_<name>`). The LED face and ear lights are not keyed in it. |

Preview mode writes `preview_hero.png`, `preview_front.png`, `preview_side.png` (512 x 512),
`preview_<animation>.png` (480 x 400, one key frame each), and `preview.png`, a contact sheet:
the three views, the key frames four to a row, then the key frames at real size (2x with 1x
under each) tinted purple, orange and silver, the last on a light background. `preview.json`
lists the order.

Full mode renders 636 frames in three passes (1,908 renders at 192 x 160).

## The app icon

`{"mode": "icon"}` renders the cat's head alone as Catbot's icon: a slight three-quarter turn,
shell in Catbot's teal (`#01A492`, the middle of the old icon's gradient), eyes open, ear lights
on, transparent background, no plate and no shadow. It writes `icon_1024.png`, `icon_512.png`,
`icon_256.png`, `icon_128.png`, `icon_64.png`, `icon_48.png`, `icon_32.png`, `icon_16.png`, and
`icon_preview.png` (the sizes from 256 down side by side on a dark and a light ground, the three
smallest also enlarged 4x).

Every size is its own render, framed automatically to fill the square. 64 px and up show the
full head: LED matrix, seams, ear lights at the tips, ears tipped out a little. 48 px and below
use a simplified head: eyes a quarter larger with no gaps between the LEDs, ear lights covering
the top third of each ear, no fine seams, ears tipped further out so the head is larger in the
square, a squarer-on view, and rendered at 8x then averaged down so edges stay crisp.

`catbot.ico`, the hicolor PNGs and the Android inset are made from these by hand.

## The in-app marks

`{"mode": "marks"}` renders two bot marks (avatars) from the cat, each a transparent square in
the pet's three passes so the app can tint the shell with the bot's colour the same way:

- `head`: the head alone in the icon's view, eyes open and cyan, ear lights on.
- `full`: the cat sitting, turned towards the viewer, cropped close so it reads at sidebar size:
  the head (with ears) takes half the square's height, the chest and front legs fill the rest,
  and the haunches, paws and tail run off the edges (`MARK_FULL_HEAD` sets the share).

Files: `mark_<head|full>_<base|mask|gloss>_<256|64>.png`. The 256 px ones are the full model. The
64 px ones are for marks shown at 32 px and below: bigger solid eyes, larger ear lights, no fine
seams, rendered at 8x and averaged down. The head is framed automatically to fill its square
with a 3% margin.

`marks.json` lists the files and, per size, the two eye centres as `[x, y]` fractions of the
square from its top left (`near` is the eye on the viewer's side, `far` the other).
`marks_preview.png` shows both marks tinted at 256, 64, 32 and 16 px on a dark and a light
ground, with the 256 one shrunk to 32 beside the simplified 32 for comparison.

## The marks alive

`{"mode": "marklife"}` renders each mark again as a strip of eight looks side by side, in the
three passes: `marklife_<head|full>_<pass>_<256|64>.png`. The looks, left to right (`MARK_LOOKS`):
resting, its blink, a glance to the near side, a glance to the far side, at work (ears pricked,
eyes narrowed), its blink, waiting on the person (head tipped, one ear cocked, wide amber eyes),
its blink. All eight share one framing, fitted round every look, so nothing jumps when the app
steps between them (`mascots.css`, `.robot-looks`). `marklife.json` lists the files and
`marklife_preview.png` shows them tinted.

## The design

**Shape.** A long, low cat: deep chest, tucked waist, round hips, long tapering legs with small
paws, a broad haunch, a whip tail, a large head with wide-set eyes and tall ears. The body is
about 4.6 units long and stands 3.5 high at the back (6.3 to the ear tips); the frame is 10.6
units wide, so one unit is about 9 CSS px.

**Shell (tinted per bot).** Glossy plastic: head, muzzle, two ears, a neck sleeve, the torso, a
shoulder-blade plate over each front leg, a haunch plate over each hind leg, tapered leg pieces,
paws, and eight tail segments. The torso and neck are each one continuous shell that bends with
the spine (smoothly skinned), so no gaps open when the cat sits or curls; the leg, tail and head
pieces are rigid. Rendered light grey (0.86) with a clear coat so its shading carries the form.

**Untinted parts.**

| Part | Look |
| --- | --- |
| Seam lines (four round the torso, one round the neck, one across each haunch, over the head, under each ear light), nose, inner ears | Graphite, almost black `(0.030, 0.033, 0.040)`, semi-matte. The seams are inlaid lines that bend with the shell. |
| Ball joints (shoulders, elbows, wrists, hips, knees, hocks, ankles, neck, between tail segments) | Soft metal `(0.60, 0.62, 0.66)`, metallic 0.85. Small and sunk between the pieces, so only a sliver shows; shoulders, hips and neck are the largest. |
| Visor | Black gloss, wrapping the front of the head from temple to temple, with a thin metal bezel |
| LED eyes | 11 x 11 matrix of small square LEDs per eye, on the visor |
| Ear tips | The top 14% of each ear is a light |

**LED face.** One face for every bot: the visor replaces both the old dot face and the
`cat-visor` face, so the `mark` no longer changes the pet's face and no separate visor overlay
is rendered. Eye shapes:

| Shape | Used for | Looks like |
| --- | --- | --- |
| `open` | content (idle, walk, sit) | tall soft oval |
| `wide` | curious, alert (jump, air, drag); amber for needs-you | big round eye |
| `half` | working | oval with a flat lid; a bright column sweeps across it, the rest dim |
| `sleepy` | sleep | dim sagging arc |
| `happy` | happy | arch |
| `squint`, `blink` | blinks, landing, stretch | flattened oval, one-row line |

Eyes can shift one LED in any direction (a glance). Colours: cyan `(0.30, 0.88, 1.0)` normally,
amber `(1.0, 0.60, 0.10)` when the bot needs Moo. The ear lights glow faintly at rest, blink in
turn while working, pulse amber with the eyes while waiting, and nearly go out in sleep.

**Camera.** Orthographic, side-on from the cat's right, turned 18 degrees towards its front and
raised 6 degrees so the face and the far legs show. The cat faces right; mirror for left.

**Light.** Three sun lamps (a broad soft key from upper front, fill, rim from behind) and a pale
even world.
No ground and no cast shadow on the ground: the film is transparent.

## Animations

All in place; the window does the travelling. Frames are 192 x 160 (2x).

| Name | Frames | fps | Plays | What it shows |
| --- | --- | --- | --- | --- |
| `idle` | 36 | 12 | loop | Standing, breathing; tail sways, a glance with a head tilt, one ear flick, a blink. |
| `walk` | 16 | 16 | loop | Low prowl, one paw at a time (near hind, near front, far hind, far front), shoulder blades riding up over the planted leg, head low and steady, tail trailing. |
| `sit_down` | 9 | 12 | once, then `sit` | Rump folds down, chest follows and settles. |
| `sit` | 36 | 12 | loop | Leaning a little forward over straight front legs, spine curved, tail wrapped round the paws; breathing, a look to the side, tail-tip flick, slow blink. |
| `lie_down` | 11 | 12 | once, then `sleep` | Crouch, then chest and paws slide forward. |
| `sleep` | 32 | 8 | loop | Curled up: spine bent in a C that opens towards the viewer, paws tucked, tail brought round, head down by the front paws, eyes dim arcs, one slow breath per loop, an ear twitch. |
| `working` | 24 | 12 | loop | Sitting hunched, kneading with the front paws, head scanning, eye sweep, ear lights blinking in turn. |
| `waiting` | 24 | 12 | loop | Sits tall, head turned to the viewer, ears pricked, wide amber eyes pulsing, a front paw lifts and pats the air, tail-tip flick. |
| `happy` | 16 | 12 | loop | Two small bounces, tail straight up with a curled tip, head tipping side to side, arch eyes. |
| `jump` | 8 | 14 | once, then `air` | Crouch with a rump wiggle, launch off the hind legs, stretch out. |
| `air` | 8 | 12 | loop | Stretched long in the air, small flutter. |
| `land` | 10 | 14 | once, then `idle` | Front paws first, deep soft crouch, up to standing. |
| `drag` | 24 | 12 | loop | Held by the scruff: body hangs and swings, legs dangle, tail curls and swishes, eyes wide. |
| `stretch` | 30 | 12 | once, then `idle` | Front paws out, chest to the floor, rump and tail high, a shiver, back up. An idle extra. |
| `curl_up` | 14 | 12 | once, then `nap` | Stage 31. Standing to the long nap's curl: a low crouch, round and down, eyes closing. |
| `nap` | 32 | 8 | loop | The long nap: curled tighter than `sleep`, nose on the paws, tail round to the nose; one slow breath, a tail-tip twitch, an ear flick. |
| `wake` | 18 | 12 | once, then `idle` | An ear flicks, the head comes up half awake, the ears prick, up to standing. |
| `dangle` | 24 | 12 | loop | Carried by the scruff: the whole cat swings under the hand, legs limp and a beat behind, tail streaming. The pet window also leans it behind the hand. |
| `tail_chase` | 40 | 16 | once, then `idle` | Notices its tail and goes twice round on the spot after it (the pose key `spin` turns the whole cat), then a dizzy wobble. |
| `look_around` | 36 | 12 | once, then `sit` | Sitting: a look round to the viewer, then away to the far side, eyes leading, each ear flicking once. |

One-shots end exactly on the first frame's pose of what follows where that is `sit`, `sleep`
or standing, so nothing snaps. `sit_down` and `lie_down` can be played backwards to get up.

## Output format

Per animation and pass, one horizontal strip of frames, left to right:

- `base_<name>.png`: the whole cat, shell in neutral grey. Straight alpha.
- `mask_<name>.png`: white, with alpha = how much of each pixel is shell. Use it as an alpha mask.
- `gloss_<name>.png`: the shell's highlights alone, on black, same alpha as the base. Everything
  that is not shell is black.

`manifest.json`:

```json
{
  "version": 1,
  "frame": {"width": 192, "height": 160, "scale": 2, "css_width": 96, "css_height": 80},
  "passes": ["base", "mask", "gloss"],
  "ground_css_y": 77.44,
  "drag_anchor_css": [58.08, 32.12],
  "walk": {"stride_css_px": 17.23, "cycle_seconds": 1.0},
  "poses": {"sit": {"intro": "sit_down", "animation": "sit"}, "...": {}},
  "animations": {
    "idle": {"frames": 36, "fps": 12, "loop": true, "next": null,
             "sheets": {"base": "base_idle.png", "mask": "mask_idle.png", "gloss": "gloss_idle.png"}}
  }
}
```

- `ground_css_y`: where the ground is in the 96 x 80 box, from the top. The paws stand about
  2.6 px above the box's bottom edge, so the sprite may want nudging down by that much.
- `drag_anchor_css`: the scruff in the `drag` animation, where the pointer should hold the cat.
- `walk`: how far the ground moves under the cat per loop. The window should move at
  `stride_css_px / cycle_seconds` (about 17 px/s) or the paws slide; `WALK` in `pet_window.rs` is
  41 px/s today. To walk faster, play the sheet faster by the same factor.
- `poses`: what each `data-pet` pose plays: an optional one-shot intro, then the animation.

## Tinting in the UI

Three layers stacked in one box; the box clips to one frame and the strip inside it steps
sideways.

```html
<div class="pet3d" style="--frames: 36; --dur: 3s; --tint: var(--identity-purple);
     --base: url(base_idle.png); --mask: url(mask_idle.png); --gloss: url(gloss_idle.png)">
  <div class="pet3d-strip">
    <div class="pet3d-base"></div>
    <div class="pet3d-tint"></div>
    <div class="pet3d-gloss"></div>
  </div>
</div>
```

```css
.pet3d { width: 96px; height: 80px; overflow: hidden; }
.pet3d-strip {
  position: relative; height: 80px; width: calc(var(--frames) * 96px);
  isolation: isolate;                       /* blending stays inside the strip */
  animation: pet3d-run var(--dur) steps(var(--frames)) infinite;
}
.pet3d-strip > div { position: absolute; inset: 0; background-size: 100% 100%; }
.pet3d-base  { background-image: var(--base); }
.pet3d-tint  { background: var(--tint); mix-blend-mode: multiply;
               -webkit-mask: var(--mask) 0 0 / 100% 100% no-repeat;
                       mask: var(--mask) 0 0 / 100% 100% no-repeat; }
.pet3d-gloss { background-image: var(--gloss); mix-blend-mode: screen; }
@keyframes pet3d-run { to { transform: translateX(-100%); } }
```

- `--dur` is `frames / fps`. For a one-shot use `animation-iteration-count: 1`, `steps(N,
  jump-none)` with `to { transform: translateX(calc(-100% + 96px)) }` and
  `animation-fill-mode: forwards`, then switch to `next` on `animationend`.
- The multiply turns the grey shell into the bot's colour with its shading; the screen layer puts
  the white highlights back, which multiply alone would dye. Dropping the gloss layer still
  works, the plastic just looks less shiny.
- At the cat's antialiased outline the tint layer can add a faint fringe of the bot's colour. If
  it shows, mask the whole strip with the base sheet too (`mask: var(--base) ...` on
  `.pet3d-strip`).
- Mirroring for walking left: `transform: scaleX(-1)` on `.pet3d`, as now.
- Light tints (silver, chrome, yellow) keep the shell bright; dark tints darken it by the same
  multiply, so no per-colour light/dark variants are needed.

## Wiring into `pet_window.rs`

- `PetSprite` becomes the markup above; the sheet names and frame counts come from the manifest
  (embed it, or generate constants from it).
- `fn pose` can stay. On a pose change, play `poses[pose].intro` if there is one, then the loop.
  `Act::Fall` could skip the `jump` intro and go straight to `air`.
- `land` runs 0.71 s and `jump` 0.57 s; `LANDING` is 0.35 s and `LEAP` 0.7 s today. Either change
  the constants or the fps.
- `happy` and `stretch` are not chosen by `fn pose` today; `stretch` suits an occasional idle
  break, `happy` a click or a finished task.
- The `mark` argument is no longer needed for the face.

## Changing things

- Poses are flat dicts of numbers (`STAND`, `SIT`, `LOAF`, ...); `P(base, key=value)` makes a
  variant. Legs are placed by paw position and solved, so moving the body keeps the paws planted.
  A leg reaches about 2.3 units; a paw put further from its shoulder or hip than that floats.
- The body's height is `BZ`; the tail's length is `TAIL_N` x `TAIL_SEG` (tail shapes are given as
  short lists and spread over however many segments there are).
- Each animation is a function of time 0..1 returning the pose, the face and the ear lights.
- After changing a pose, `python3 art/pet3d/soft_preview.py /tmp/out [animation ...]` draws it
  and prints each animation's bounds in the 96 x 80 box.

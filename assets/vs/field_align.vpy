# Embedded copy of vhs-cli/vhs-env/tools/field_align.vpy. Only the plugin loading differs:
# plugins are found through vhs_plugins.py instead of absolute /home paths.
import os
import vapoursynth as vs
import vhs_plugins

core = vs.core

# Explicit plugin loads (since autoload has been unreliable)
vhs_plugins.load("libffms2.so")

SRC = os.environ["VS_INPUT"]
TFF = int(os.environ.get("VS_TFF", "1"))

# Horizontal sub-pixel shift to apply to one field to correct misalignment.
# Positive values shift the corrected field rightward; negative shifts left.
# Typical VHS values are 0.5–3.0 pixels. Start with 1.0 and adjust.
FIELD_SHIFT = float(os.environ.get("VS_FIELD_SHIFT", "1.0"))

# Which field to shift: "top" or "bottom". Default: shift bottom field.
SHIFT_FIELD = os.environ.get("VS_SHIFT_FIELD", "bottom")

clip = core.ffms2.Source(source=SRC)

# Separate into individual fields (half height each)
fields = core.std.SeparateFields(clip, tff=(TFF == 1))

# fields is now double framerate, alternating top/bottom:
#   TFF: frame 0 = top, frame 1 = bottom, frame 2 = top, ...
#   BFF: frame 0 = bottom, frame 1 = top, frame 2 = bottom, ...

if TFF == 1:
    top_idx = 0
    bot_idx = 1
else:
    top_idx = 1
    bot_idx = 0

# Determine which parity to shift
if SHIFT_FIELD == "top":
    shift_idx = top_idx
elif SHIFT_FIELD == "bottom":
    shift_idx = bot_idx
else:
    raise ValueError(f"VS_SHIFT_FIELD must be 'top' or 'bottom', got '{SHIFT_FIELD}'")

# Apply horizontal sub-pixel shift using built-in resize.Spline36.
# src_left shifts the source sampling window, effectively moving the image.
# Positive src_left = shift image rightward.
even_fields = core.std.SelectEvery(fields, cycle=2, offsets=0)
odd_fields = core.std.SelectEvery(fields, cycle=2, offsets=1)

if shift_idx == 0:
    # Shift even fields (first of each pair)
    even_fields = core.resize.Spline36(
        even_fields,
        width=even_fields.width,
        height=even_fields.height,
        src_left=FIELD_SHIFT,
    )
else:
    # Shift odd fields (second of each pair)
    odd_fields = core.resize.Spline36(
        odd_fields,
        width=odd_fields.width,
        height=odd_fields.height,
        src_left=FIELD_SHIFT,
    )

# Interleave back into alternating field order
aligned = core.std.Interleave([even_fields, odd_fields])

# Re-weave into interlaced frames
aligned = core.std.DoubleWeave(aligned, tff=(TFF == 1))

# DoubleWeave doubles the frame count — keep every other frame
aligned = core.std.SelectEvery(aligned, cycle=2, offsets=0)

aligned.set_output()

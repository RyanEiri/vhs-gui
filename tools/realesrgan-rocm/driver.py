#!/usr/bin/env python3
"""
Drop-in ROCm/PyTorch backend for realesrgan-ncnn-vulkan.
Accepts the same CLI flags; resolves model name to a .pth checkpoint via MODEL_MAP.

Batching: set BATCH_SIZE env var (default 8) to process N frames per GPU call.
At 720x480 VHS source in fp16, BATCH_SIZE=16-32 keeps the GPU well-fed without
exhausting 16GB VRAM (each input batch ~50MB, output ~1.6GB at BATCH_SIZE=16).
"""
import argparse
import os
import sys
from pathlib import Path

import numpy as np
import torch
from PIL import Image
from spandrel import ImageModelDescriptor, ModelLoader

MODEL_MAP = {
    "realesrgan-x4plus":          "RealESRGAN_x4plus.pth",
    "realesrgan-x4plus-anime":    "RealESRGAN_x4plus_anime_6B.pth",
    "realesrgan-x2plus":          "RealESRGAN_x2plus.pth",
    "2x_VHS-Film":                "2x_VHS-upscale-and-denoise_Film_477000_G.pth",
    "ToonVHS-1x":                 "ToonVHS-1x-300000_G.pth",
    "VHS-Sharpen-1x":             "VHS-Sharpen-1x_46000_G.pth",
}

JPEG_QUALITY = 95


def parse_args():
    p = argparse.ArgumentParser(
        description="ROCm Real-ESRGAN upscaler (realesrgan-ncnn-vulkan drop-in)"
    )
    p.add_argument("-i", required=True, help="Input frames directory")
    p.add_argument("-o", required=True, help="Output frames directory")
    p.add_argument("-n", required=True, help="Model name (NCNN convention)")
    p.add_argument("-m", required=True, help="Models directory (unused; checkpoints resolved from script dir)")
    p.add_argument("-s", type=int, default=4, help="Scale factor")
    p.add_argument("-t", type=int, default=0,
                   help="Tile size (0=no tiling, default for ROCm — full frame fits in VRAM at VHS res)")
    p.add_argument("-j", default="3:3:3", help="Threads load:proc:save (proc ignored)")
    p.add_argument("-g", type=int, default=0, help="GPU/HIP device index")
    p.add_argument("-f", default="jpg", help="Output format extension")
    return p.parse_args()


def load_model(model_name: str, device: torch.device) -> ImageModelDescriptor:
    if model_name not in MODEL_MAP:
        print(
            f"error: model '{model_name}' is not available in the ROCm backend.\n"
            f"  Available: {', '.join(MODEL_MAP)}\n"
            f"  For other models, use UPSCALE_BACKEND=vulkan.",
            file=sys.stderr,
        )
        sys.exit(2)

    script_dir = Path(__file__).parent
    ckpt_path = script_dir / "models" / MODEL_MAP[model_name]
    if not ckpt_path.exists():
        print(
            f"error: checkpoint not found: {ckpt_path}\n"
            f"  Download it with:\n"
            f"    wget -P {ckpt_path.parent} "
            f"https://github.com/xinntao/Real-ESRGAN/releases/download/v0.1.0/{MODEL_MAP[model_name]}",
            file=sys.stderr,
        )
        sys.exit(2)

    dtype_name = os.environ.get("ROCM_DTYPE", "fp16").lower()
    dtype = torch.float16 if dtype_name == "fp16" else torch.float32

    loader = ModelLoader()
    descriptor = loader.load_from_file(str(ckpt_path))
    if not isinstance(descriptor, ImageModelDescriptor):
        print("error: loaded model is not a single-image descriptor", file=sys.stderr)
        sys.exit(2)

    descriptor.model.to(device=device, dtype=dtype).eval()
    return descriptor


def tile_infer(
    model: ImageModelDescriptor,
    img_tensor: torch.Tensor,
    tile_size: int,
    scale: int,
    device: torch.device,
) -> torch.Tensor:
    """Tile-based inference for a single image [1, C, H, W]."""
    pad = 10
    stride = tile_size - pad * 2
    b, c, h, w = img_tensor.shape
    out_h, out_w = h * scale, w * scale
    dtype = img_tensor.dtype

    output = torch.zeros(b, c, out_h, out_w, dtype=dtype, device=device)
    weights = torch.zeros(b, 1, out_h, out_w, dtype=dtype, device=device)

    for y in range(0, h, stride):
        for x in range(0, w, stride):
            y_end = min(y + tile_size, h)
            x_end = min(x + tile_size, w)
            tile = img_tensor[:, :, y:y_end, x:x_end]
            with torch.inference_mode():
                out_tile = model(tile)
            oy, ox = y * scale, x * scale
            output[:, :, oy:oy + out_tile.shape[2], ox:ox + out_tile.shape[3]] += out_tile
            weights[:, :, oy:oy + out_tile.shape[2], ox:ox + out_tile.shape[3]] += 1

    return output / weights


def load_frame(path: Path, device: torch.device, dtype: torch.dtype) -> torch.Tensor:
    arr = np.array(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
    return torch.from_numpy(arr).permute(2, 0, 1).to(device=device, dtype=dtype)


def save_frame(tensor: torch.Tensor, path: Path, ext: str):
    arr = tensor.permute(1, 2, 0).float().clamp(0, 1).cpu().numpy()
    Image.fromarray((arr * 255).astype(np.uint8)).save(str(path), quality=JPEG_QUALITY)


def process_batch(
    paths: list,
    out_dir: Path,
    model: ImageModelDescriptor,
    tile_size: int,
    scale: int,
    device: torch.device,
    out_ext: str,
):
    dtype = next(model.model.parameters()).dtype

    if tile_size > 0:
        # Tiled path: process one at a time (tiling doesn't batch cleanly)
        for p in paths:
            t = load_frame(p, device, dtype).unsqueeze(0)
            with torch.inference_mode():
                out = tile_infer(model, t, tile_size, scale, device).squeeze(0)
            save_frame(out, out_dir / (p.stem + "." + out_ext), out_ext)
    else:
        # Full-frame batched path: stack all frames, one GPU call
        batch = torch.stack([load_frame(p, device, dtype) for p in paths])  # [N, C, H, W]
        with torch.inference_mode():
            out_batch = model(batch)  # [N, C, H*s, W*s]
        for i, p in enumerate(paths):
            save_frame(out_batch[i], out_dir / (p.stem + "." + out_ext), out_ext)


def main():
    args = parse_args()

    batch_size = int(os.environ.get("BATCH_SIZE", "8"))

    parts = args.j.split(":")
    try:
        _ = int(parts[0])  # load threads (unused, PIL is fast)
    except (ValueError, IndexError):
        pass

    os.environ["HIP_VISIBLE_DEVICES"] = str(args.g)
    device = torch.device(f"cuda:{args.g}" if torch.cuda.is_available() else "cpu")
    if device.type == "cpu":
        print("warning: ROCm GPU not detected, falling back to CPU (slow)", file=sys.stderr)

    model = load_model(args.n, device)

    in_dir = Path(args.i)
    out_dir = Path(args.o)
    out_dir.mkdir(parents=True, exist_ok=True)

    frames = sorted(in_dir.glob(f"*.{args.f}"))
    if not frames:
        frames = sorted(in_dir.glob("*.jpg"))
    if not frames:
        print(f"error: no frames found in {in_dir}", file=sys.stderr)
        sys.exit(1)

    total = len(frames)
    mode = f"tile={args.t}" if args.t > 0 else f"batch={batch_size} (full-frame)"
    print(f"  [{args.n}] {total} frames  {mode}  scale={args.s}x  device={device}", flush=True)

    done = 0
    for start in range(0, total, batch_size):
        chunk = frames[start:start + batch_size]
        process_batch(chunk, out_dir, model, args.t, args.s, device, args.f)
        done += len(chunk)
        if done % 200 == 0 or done == total:
            print(f"  {done}/{total}", flush=True)


if __name__ == "__main__":
    main()

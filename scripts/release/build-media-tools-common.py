"""Shared, hash-locked FFmpeg source build helpers used by native CI builders."""
import hashlib
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "scripts/release/locked-tools.json").read_text(encoding="utf-8"))


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_file(name: str, url: str, expected: str) -> pathlib.Path:
    cache = ROOT / ".cache/media-tool-sources"
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / name
    if not path.is_file() or sha256(path) != expected:
        path.unlink(missing_ok=True)
        request = urllib.request.Request(url, headers={"User-Agent": "Burrata-release-tool-preparer"})
        with urllib.request.urlopen(request, timeout=180) as response, path.open("wb") as output:
            shutil.copyfileobj(response, output)
    actual = sha256(path)
    if actual != expected:
        path.unlink(missing_ok=True)
        raise RuntimeError(f"SHA-256 mismatch for {name}: expected {expected}, got {actual}")
    return path


def extract(archive: pathlib.Path, directory: pathlib.Path) -> pathlib.Path:
    directory.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:*") as source:
        source.extractall(directory, filter="data")
    roots = list(directory.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise RuntimeError(f"Unexpected source archive layout: {archive.name}")
    return roots[0]


def run(args: list[str], cwd: pathlib.Path, env: dict[str, str], *, windows: bool = False) -> None:
    if windows:
        bash = env.get("BURRATA_MSYS2_BASH")
        if not bash or not pathlib.Path(bash).is_file():
            raise RuntimeError("Windows media builds require BURRATA_MSYS2_BASH to name the active MSYS2 bash.exe")
        command = " ".join(shlex.quote(item) for item in args)
        shell_cwd = posix_path(cwd)
        command = f"cd {shlex.quote(shell_cwd)} && {command}"
        subprocess.run([bash, "-lc", command], cwd=ROOT, env=env, check=True)
    else:
        subprocess.run(args, cwd=cwd, env=env, check=True)


def posix_path(path: pathlib.Path) -> str:
    if os.name != "nt":
        return str(path)
    return subprocess.check_output(["cygpath", "-u", str(path)], text=True).strip()


def prepare_sources(dest: pathlib.Path, target: str, names: tuple[str, ...]) -> tuple[pathlib.Path, dict[str, pathlib.Path], list[dict[str, str]]]:
    tmp = pathlib.Path(os.environ.get("BURRATA_MEDIA_BUILD_DIR", ".cache/media-build")) / target
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True, exist_ok=True)
    source_dir = tmp / "sources"
    source_dir.mkdir()
    prefix = tmp / "prefix"
    prefix.mkdir()
    specs = {
        "ffmpeg": (LOCK["ffmpeg"]["source"]["archive"], LOCK["ffmpeg"]["source"]["url"], LOCK["ffmpeg"]["source"]["sha256"], LOCK["ffmpeg"]["version"]),
        "lame": ("lame-3.100.tar.gz", LOCK["lame"]["url"], LOCK["lame"]["sha256"], LOCK["lame"]["version"]),
        "opus": ("opus-1.6.1.tar.gz", LOCK["opus"]["url"], LOCK["opus"]["sha256"], LOCK["opus"]["version"]),
        "zlib": ("zlib-1.3.2.tar.gz", LOCK["zlib"]["url"], LOCK["zlib"]["sha256"], LOCK["zlib"]["version"]),
        "openssl": ("openssl-3.5.9.tar.gz", LOCK["openssl"]["url"], LOCK["openssl"]["sha256"], LOCK["openssl"]["version"]),
    }
    extracted: dict[str, pathlib.Path] = {}
    inventory: list[dict[str, str]] = []
    for key in names:
        filename, url, digest, version = specs[key]
        archive = source_file(filename, url, digest)
        shutil.copy2(archive, source_dir / filename)
        extracted[key] = extract(archive, tmp / f"{key}-source")
        inventory.append({"name": key, "version": version, "url": url, "sha256": digest, "file": filename})
    destination_sources = dest / "sources"
    destination_sources.mkdir(parents=True, exist_ok=True)
    for item in inventory:
        shutil.copy2(source_dir / item["file"], destination_sources / item["file"])
    return prefix, extracted, inventory


def build_libs(prefix: pathlib.Path, extracted: dict[str, pathlib.Path], jobs: str, env: dict[str, str], *, windows: bool) -> None:
    lame = extracted["lame"]
    host = ["--host=x86_64-w64-mingw32"] if windows else []
    configure = ["sh", "./configure"] if windows else ["./configure"]
    run([*configure, f"--prefix={posix_path(prefix)}", *host, "--enable-static", "--disable-shared", "--disable-frontend"], lame, env, windows=windows)
    run(["make", "-j" + jobs], lame, env, windows=windows)
    run(["make", "install"], lame, env, windows=windows)

    opus = extracted["opus"]
    run([*configure, f"--prefix={posix_path(prefix)}", *host, "--enable-static", "--disable-shared"], opus, env, windows=windows)
    run(["make", "-j" + jobs], opus, env, windows=windows)
    run(["make", "install"], opus, env, windows=windows)

    zlib = extracted["zlib"]
    run([*configure, "--static", f"--prefix={posix_path(prefix)}"], zlib, env, windows=windows)
    run(["make", "-j" + jobs], zlib, env, windows=windows)
    run(["make", "install"], zlib, env, windows=windows)


def stage_notices(dest: pathlib.Path, extracted: dict[str, pathlib.Path], inventory: list[dict[str, str]]) -> None:
    licenses = {
        "lame": ("COPYING", "LAME_LICENSE.txt"),
        "opus": ("COPYING", "OPUS_LICENSE.txt"),
        "zlib": ("LICENSE", "ZLIB_LICENSE.txt"),
        "openssl": ("LICENSE.txt", "OPENSSL_LICENSE.txt"),
    }
    for key, (source_name, output_name) in licenses.items():
        if key not in extracted:
            continue
        license_path = extracted[key] / source_name
        if not license_path.is_file():
            raise RuntimeError(f"Missing upstream license file: {license_path}")
        shutil.copy2(license_path, dest / output_name)
    (dest / "MEDIA_SOURCE_INVENTORY.json").write_text(json.dumps({"sources": inventory}, indent=2) + "\n", encoding="utf-8")

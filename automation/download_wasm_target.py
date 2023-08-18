import subprocess
import requests
import tempfile
import os
import tarfile
import shutil
from pathlib import Path


def get_rustc_version():
    command = ["rustc", "--version"]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError("Error executing rustc --version")

    version_line = result.stdout.strip()
    return version_line.split()[1].strip()


def ensure_wasm_toolchain():
    package_root = os.environ.get("PIXI_PACKAGE_ROOT", None)
    if package_root is None:
        raise ValueError("Expected PIXI_PACKAGE_ROOT environment variable to be set")

    dest_dir = os.path.join(
        package_root, ".pixi", "env", "lib", "rustlib", "wasm32-unknown-unknown"
    )
    if os.path.exists(dest_dir):
        print("wasm32-unknown-unknown target already installed")
        return

    rustc_version = get_rustc_version()
    url = f"https://static.rust-lang.org/dist/rust-std-{rustc_version}-wasm32-unknown-unknown.tar.gz"
    print(f"Downloading wasm32-unknown-unknown toolchain from {url}")
    result = requests.get(url)

    # Check if the download was successful
    if result.status_code != 200:
        raise Exception(
            f"Failed to download toolchain target. HTTP Status Code: {result.status_code}"
        )

    # Create a temporary directory
    with tempfile.TemporaryDirectory() as temp_dir:
        # Write the downloaded content to a temporary file
        temp_file_path = os.path.join(
            temp_dir, f"rust-std-{rustc_version}-wasm32-unknown-unknown.tar.gz"
        )
        with open(temp_file_path, "wb") as temp_file:
            temp_file.write(result.content)

        # Extract the contents of the tarball
        with tarfile.open(temp_file_path, 'r:gz') as tar:
            tar.extractall(path=temp_dir)

        # Define the source and destination directories
        src_dir = os.path.join(
            temp_dir,
            f"rust-std-{rustc_version}-wasm32-unknown-unknown",
            "rust-std-wasm32-unknown-unknown",
            "lib",
            "rustlib",
            "wasm32-unknown-unknown"
        )

        # Copy the directory
        if os.path.exists(dest_dir):
            shutil.rmtree(dest_dir)
        shutil.copytree(src_dir, dest_dir)
        print(f"Installed wasm toolchain to {dest_dir}")


if __name__ == '__main__':
    ensure_wasm_toolchain()

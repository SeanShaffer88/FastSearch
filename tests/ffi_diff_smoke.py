"""Exercise catalog comparison through a compiled FastSearch library."""
import ctypes
import json
from pathlib import Path
import sys
import tempfile


def main():
    lib = ctypes.CDLL(str(Path(sys.argv[1]).resolve()))
    lib.fs_index_load.argtypes = [ctypes.c_char_p]
    lib.fs_index_load.restype = ctypes.c_uint64
    lib.fs_index_diff_json.argtypes = [ctypes.c_uint64, ctypes.c_uint64, ctypes.c_char_p, ctypes.c_int]
    lib.fs_index_diff_json.restype = ctypes.c_void_p
    lib.fs_free_string.argtypes = [ctypes.c_void_p]
    lib.fs_index_free.argtypes = [ctypes.c_uint64]
    lib.fs_index_free.restype = ctypes.c_int
    lib.fs_last_error_code.restype = ctypes.c_int
    with tempfile.TemporaryDirectory(prefix="fastsearch-diff-") as folder:
        before_path = Path(folder) / "before.fsidx"
        after_path = Path(folder) / "after.fsidx"
        before_path.write_text("FSIDX1\nF C:/old.dll\nF C:/same.dll\nD C:/kind.dll\n", encoding="utf-8")
        after_path.write_text("FSIDX1\nF C:/new.dll\nF C:/same.dll\nF C:/kind.dll\n", encoding="utf-8")
        before = lib.fs_index_load(str(before_path).encode())
        after = lib.fs_index_load(str(after_path).encode())
        if not before or not after:
            raise RuntimeError("Could not load synthetic catalogs")
        try:
            ptr = lib.fs_index_diff_json(before, after, b"*.dll", 10)
            if not ptr:
                raise RuntimeError(f"Diff failed: error {lib.fs_last_error_code()}")
            try:
                report = json.loads(ctypes.string_at(ptr).decode())
            finally:
                lib.fs_free_string(ptr)
            expected = {
                "schema_version": 1, "filesystem_verified": False,
                "added_to_index": 1, "missing_from_index": 1,
                "kind_changed": 1, "unchanged": 1,
                "details_truncated": False,
                "changes": [
                    {"path": "C:/kind.dll", "change": "kind_changed", "before_folder": True, "after_folder": False},
                    {"path": "C:/new.dll", "change": "added_to_index", "before_folder": None, "after_folder": False},
                    {"path": "C:/old.dll", "change": "missing_from_index", "before_folder": False, "after_folder": None},
                ],
            }
            if report != expected:
                raise RuntimeError(f"Unexpected diff JSON: {report}")
            ptr = lib.fs_index_diff_json(before, after, b"", 1)
            if not ptr:
                raise RuntimeError("Blank-query diff failed")
            try:
                limited = json.loads(ctypes.string_at(ptr).decode())
            finally:
                lib.fs_free_string(ptr)
            if limited["unchanged"] != 1 or len(limited["changes"]) != 1 or not limited["details_truncated"]:
                raise RuntimeError(f"Detail limit did not preserve totals: {limited}")
            if lib.fs_index_diff_json(0, after, b"", 10) or lib.fs_last_error_code() != 3:
                raise RuntimeError("Invalid handle error contract failed")
            if lib.fs_index_diff_json(before, after, None, 10) or lib.fs_last_error_code() != 1:
                raise RuntimeError("Null query error contract failed")
        finally:
            lib.fs_index_free(before)
            lib.fs_index_free(after)
    print("FFI diff smoke passed: offline comparison, classifications, filtering, detail truncation, and errors")


if __name__ == "__main__":
    main()

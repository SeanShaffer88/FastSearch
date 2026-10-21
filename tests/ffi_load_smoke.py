"""Run against a built DLL/shared library, including panic=abort release builds.
Usage: python tests/ffi_load_smoke.py /absolute/path/to/fast_search.dll
"""
import ctypes
from pathlib import Path
import sys
import tempfile


def main():
    lib = ctypes.CDLL(str(Path(sys.argv[1]).resolve()))
    lib.fs_index_load.argtypes = [ctypes.c_char_p]
    lib.fs_index_load.restype = ctypes.c_uint64
    lib.fs_index_count.argtypes = [ctypes.c_uint64]
    lib.fs_index_count.restype = ctypes.c_int64
    lib.fs_index_free.argtypes = [ctypes.c_uint64]
    lib.fs_index_free.restype = None
    lib.fs_last_error_code.restype = ctypes.c_int
    with tempfile.TemporaryDirectory(prefix='fastsearch-load-') as folder:
        path = Path(folder) / 'index.fsidx'
        for line in ['雪', '😀', 'a雪', 'F雪', 'é/path']:
            path.write_text(f'FSIDX1\n{line}\n', encoding='utf-8')
            if lib.fs_index_load(str(path).encode('utf-8')) != 0:
                raise RuntimeError('Malformed record was accepted')
            if lib.fs_last_error_code() != 5:
                raise RuntimeError('Malformed record did not return ERR_PLATFORM')
        path.write_text('FSIDX1\nF C:/雪/😀.txt\nD C:/café\n', encoding='utf-8')
        handle = lib.fs_index_load(str(path).encode('utf-8'))
        if not handle:
            raise RuntimeError('Valid Unicode index was rejected')
        try:
            if lib.fs_index_count(handle) != 2 or lib.fs_last_error_code() != 0:
                raise RuntimeError('Valid index count/error contract failed')
        finally:
            lib.fs_index_free(handle)
    print('FFI load smoke passed: malformed Unicode rejected; valid Unicode accepted')


if __name__ == '__main__':
    main()

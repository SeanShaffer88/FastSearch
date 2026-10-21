"""Run against a built DLL/shared library, including panic=abort release builds.
Usage: python tests/ffi_load_smoke.py /absolute/path/to/fast_search.dll
"""
import ctypes
import json
from pathlib import Path
import sys
import tempfile


def main():
    lib = ctypes.CDLL(str(Path(sys.argv[1]).resolve()))
    lib.fs_index_load.argtypes = [ctypes.c_char_p]
    lib.fs_index_load.restype = ctypes.c_uint64
    lib.fs_index_count.argtypes = [ctypes.c_uint64]
    lib.fs_index_count.restype = ctypes.c_int64
    lib.fs_index_build_report_json.argtypes = [ctypes.c_uint64, ctypes.c_char_p]
    lib.fs_index_build_report_json.restype = ctypes.c_void_p
    control_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p)
    lib.fs_index_build_controlled_report_json.argtypes = [ctypes.c_uint64, ctypes.c_char_p, control_type, ctypes.c_void_p]
    lib.fs_index_build_controlled_report_json.restype = ctypes.c_void_p
    lib.fs_free_string.argtypes = [ctypes.c_void_p]
    lib.fs_free_string.restype = None
    lib.fs_index_build.argtypes = [ctypes.c_uint64, ctypes.c_char_p]
    lib.fs_index_build.restype = ctypes.c_int
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
            if lib.fs_index_build(handle, str(Path(folder) / 'missing-root').encode('utf-8')) == 0:
                raise RuntimeError('Invalid walk root was accepted')
            if lib.fs_index_count(handle) != 2:
                raise RuntimeError('Failed index build replaced the existing index')
            scan = Path(folder) / 'scan'
            scan.mkdir()
            (scan / 'sample.txt').write_text('synthetic')
            pointer = lib.fs_index_build_report_json(handle, str(scan).encode('utf-8'))
            if not pointer:
                raise RuntimeError('Build report failed')
            try:
                report = json.loads(ctypes.string_at(pointer).decode('utf-8'))
            finally:
                lib.fs_free_string(pointer)
            (scan / 'new-entry.txt').write_text('must not appear until commit')
            seen = []
            @control_type
            def cancel(examined, context):
                seen.append(examined)
                return int(examined >= 1)
            result = lib.fs_index_build_controlled_report_json(handle, str(scan).encode('utf-8'), cancel, None)
            if result or lib.fs_last_error_code() != 7 or seen != [0, 1]:
                raise RuntimeError('Controlled scan cancellation contract failed')
            if lib.fs_index_count(handle) != 1:
                raise RuntimeError('Cancellation replaced the previous index')
            @control_type
            def proceed(examined, context):
                return 0
            result = lib.fs_index_build_controlled_report_json(handle, str(scan).encode('utf-8'), proceed, None)
            if not result:
                raise RuntimeError('Controlled scan could not complete')
            lib.fs_free_string(result)
            expected = dict(schema_version=1, indexed_entries=1, unreadable_dirs=0,
                            unreadable_entries=0, skipped_links=0, partial=False)
            if report != expected or lib.fs_index_count(handle) != 2:
                raise RuntimeError(f'Unexpected build report: {report}')
        finally:
            lib.fs_index_free(handle)
    print('FFI load smoke passed: malformed Unicode rejected; valid Unicode accepted; failed build preserves index; build report and cancellation verified')


if __name__ == '__main__':
    main()

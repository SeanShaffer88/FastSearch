"""Usage: python search.py path/to/fast_search.dll disposable-folder [query]"""
import ctypes, json, pathlib, sys
lib=ctypes.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
lib.fs_index_create.restype=ctypes.c_uint64
lib.fs_index_build_report_json.argtypes=[ctypes.c_uint64,ctypes.c_char_p];lib.fs_index_build_report_json.restype=ctypes.c_void_p
lib.fs_search_json.argtypes=[ctypes.c_uint64,ctypes.c_char_p,ctypes.c_int];lib.fs_search_json.restype=ctypes.c_void_p
lib.fs_free_string.argtypes=[ctypes.c_void_p];lib.fs_free_string.restype=None
lib.fs_index_free.argtypes=[ctypes.c_uint64];lib.fs_index_free.restype=ctypes.c_int
lib.fs_last_error.restype=ctypes.c_char_p
lib.fs_last_error_code.restype=ctypes.c_int
handle=lib.fs_index_create()
if not handle:raise RuntimeError('index creation failed')
def take(pointer):
    if not pointer:raise RuntimeError(f'{lib.fs_last_error_code()}: {lib.fs_last_error().decode("utf-8")}')
    try:return json.loads(ctypes.string_at(pointer).decode('utf-8'))
    finally:lib.fs_free_string(pointer)
try:
    report=take(lib.fs_index_build_report_json(handle,str(pathlib.Path(sys.argv[2]).resolve()).encode('utf-8')))
    print(json.dumps({'build':report},ensure_ascii=False))
    results=take(lib.fs_search_json(handle,(sys.argv[3] if len(sys.argv)>3 else '*.txt').encode('utf-8'),100))
    print(json.dumps({'results':results},ensure_ascii=False))
finally:
    if lib.fs_index_free(handle)!=0:raise RuntimeError('handle release failed')

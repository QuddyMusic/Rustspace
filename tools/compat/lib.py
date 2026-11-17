import json, re, urllib.request, asyncio, websockets
def http(port, method, path, body=None, token=None):
    req = urllib.request.Request(f'http://127.0.0.1:{port}{path}', method=method)
    if token: req.add_header('Authorization', 'Bearer '+token)
    data = None
    if body is not None:
        data = json.dumps(body).encode(); req.add_header('Content-Type','application/json')
    try:
        with urllib.request.urlopen(req, data, timeout=20) as r:
            raw = r.read(); return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try: return e.code, json.loads(raw)
        except Exception: return e.code, raw.decode(errors='replace')
async def ws_session(port, token, sends=(), wait=1.0, first_only=False):
    out = []
    async with websockets.connect(f'ws://127.0.0.1:{port}/ws', max_size=None) as ws:
        await ws.send(json.dumps({'type':'auth','token':token}))
        out.append(json.loads(await asyncio.wait_for(ws.recv(), 20)))
        if first_only: return out
        for s in sends:
            await ws.send(json.dumps(s))
        try:
            while True:
                out.append(json.loads(await asyncio.wait_for(ws.recv(), wait)))
        except asyncio.TimeoutError:
            pass
    return out
def ready(port, token):
    return asyncio.run(ws_session(port, token, first_only=True))[0]
def norm(v):
    """Порядок массивов объектов с id не важен: сортируем."""
    if isinstance(v, dict): return {k: norm(x) for k, x in v.items()}
    if isinstance(v, list):
        items = [norm(x) for x in v]
        if items and all(isinstance(x, dict) for x in items):
            key = lambda x: str(x.get('id') or x.get('userId') or x.get('channelId') or json.dumps(x, sort_keys=True))
            items.sort(key=key)
        return items
    return v
# Осознанные отличия Rustspace от оригинала: своя версия, поле совместимости
# и имя инстанса по умолчанию.
DELIBERATE = ('.version', '.compatibleWith', '.name', '.instanceName')

def diff(a, b, path='', out=None, ignore=()):
    out = [] if out is None else out
    if any(path.endswith(i) for i in ignore): return out
    if any(path.endswith(i) for i in DELIBERATE) and path.count('.') == 1: return out
    if path.endswith('.avatarColor') and re.search(r'members\[|user|moderator|friend|\]\.avatarColor$', path) and 'space' not in path.split('.')[-2]: return out
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a)|set(b)):
            if k not in a: out.append(f'{path}.{k}: нет у оригинала, у rust {json.dumps(b[k])[:80]}')
            elif k not in b: out.append(f'{path}.{k}: нет у rust, у оригинала {json.dumps(a[k])[:80]}')
            else: diff(a[k], b[k], f'{path}.{k}', out, ignore)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b): out.append(f'{path}: длина {len(a)} != {len(b)}')
        for i,(x,y) in enumerate(zip(a,b)): diff(x, y, f'{path}[{i}]', out, ignore)
    elif a != b or type(a) != type(b):
        out.append(f'{path}: {json.dumps(a)[:80]} != {json.dumps(b)[:80]}')
    return out

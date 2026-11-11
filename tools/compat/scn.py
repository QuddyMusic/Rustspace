"""Сценарии совместимости: одни и те же запросы к оригиналу и к Rust-серверу,
ответы и события WebSocket сравниваются после нормализации идентификаторов."""
import sys, json, re, time, threading, asyncio, random, string
sys.path.insert(0, '/tmp/stand')
from lib import http, diff
import websockets

TS_KEYS = re.compile(r'(At|timestamp|Timestamp|expires|Expires)$')
class Norm:
    def __init__(self): self.ids = {}
    def tok(self, s):
        if s not in self.ids: self.ids[s] = f'<id{len(self.ids)+1}>'
        return self.ids[s]
    def text(self, s):
        s = re.sub(r'\d{15,}', lambda m: self.tok(m.group(0)), s)
        return s
    def val(self, v, key=''):
        if isinstance(v, dict): return {self.text(k): self.val(v[k], k) for k in sorted(v)}
        if isinstance(v, list): return [self.val(x, key) for x in v]
        if isinstance(v, str):
            if key in ('inviteCode', 'token', 'code') and v: return self.tok(v) if key != 'token' else '<token>'
            return self.text(v)
        if isinstance(v, (int, float)) and not isinstance(v, bool) and TS_KEYS.search(key) and v > 1_600_000_000_000: return '<ts>'
        return v

class Listener(threading.Thread):
    def __init__(self, port, token):
        super().__init__(daemon=True); self.port, self.token = port, token
        self.events = []; self.ready = threading.Event(); self.stop = False; self.outq = []
        self.start(); self.ready.wait(20)
    def run(self): asyncio.run(self.main())
    async def main(self):
        async with websockets.connect(f'ws://127.0.0.1:{self.port}/ws', max_size=None) as ws:
            await ws.send(json.dumps({'type': 'auth', 'token': self.token}))
            self.first = json.loads(await ws.recv()); self.ready.set()
            while not self.stop:
                while self.outq: await ws.send(json.dumps(self.outq.pop(0)))
                try: self.events.append(json.loads(await asyncio.wait_for(ws.recv(), 0.05)))
                except asyncio.TimeoutError: pass
                except Exception: break
    def send(self, event): self.outq.append(event)

class Ctx:
    def __init__(self, port, tag):
        self.port, self.tag = port, tag; self.n = Norm(); self.log = []; self.tok = {}; self.uid = {}; self.ws = {}; self.v = {}
    def user(self, name, listen=True):
        full = f'{name}{self.tag}'
        s, b = http(self.port, 'POST', '/api/auth/register', {'username': full, 'password': 'password123'})
        assert s in (200, 201), (s, b)
        self.tok[name] = b['token']; self.uid[name] = b['user']['id']; self.n.tok(b['user']['id'])
        if listen: self.ws[name] = Listener(self.port, b['token'])
        return b['user']['id']
    def listen(self, name): self.ws[name] = Listener(self.port, self.tok[name])
    def call(self, who, method, path, body=None, label=None):
        s, b = http(self.port, method, path, body, self.tok.get(who))
        self.log.append((label or f'{who} {method} {self.n.text(path)}', s, self.n.val(b)))
        return b if isinstance(b, (dict, list)) else {}
    def send(self, who, event): self.ws[who].send(event)
    def settle(self, t=0.5): time.sleep(t)
    def events(self):
        out = {}
        for name, l in self.ws.items():
            out[name] = [self.n.val(e) for e in l.events]
        return out
    def stop(self, name):
        self.settle(); self.ws[name].stop = True
    def close(self):
        for l in self.ws.values(): l.stop = True

RANDOM = ('.user.avatarColor', '.moderator.avatarColor', '.members[0].avatarColor')
def run(scenario, ports=(3105, 3106), show=False, ignore=RANDOM):
    tag = ''.join(random.choice(string.ascii_lowercase) for _ in range(5))
    res = []
    for p in ports:
        c = Ctx(p, tag); scenario(c); c.settle(0.8); res.append((c.log, c.events())); c.close()
    (la, ea), (lb, eb) = res
    bad = 0
    for i, (x, y) in enumerate(zip(la, lb)):
        d = [] if x[1] == y[1] else [f'статус {x[1]} != {y[1]}  тело ориг: {json.dumps(x[2], ensure_ascii=False)[:300]}  rust: {json.dumps(y[2], ensure_ascii=False)[:300]}']
        d += diff(x[2], y[2], ignore=ignore)
        if d:
            bad += 1; print(f'✗ [{i}] {x[0]}'); [print('    ', l) for l in d[:12]]
        elif show: print(f'✓ [{i}] {x[0]} → {x[1]}')
    for name in ea:
        a, b = ea[name], eb.get(name, [])
        ta, tb = [e.get('type') for e in a], [e.get('type') for e in b]
        if ta != tb:
            bad += 1; print(f'✗ события {name}:\n     ориг: {ta}\n     rust: {tb}')
        else:
            for i, (x, y) in enumerate(zip(a, b)):
                d = diff(x, y, ignore=ignore)
                if d: bad += 1; print(f'✗ событие {name}[{i}] {x.get("type")}'); [print('    ', l) for l in d[:12]]
            if show: print(f'✓ события {name}: {ta}')
    print(f'{scenario.__name__}: шагов {len(la)}, расхождений {bad}')
    return bad

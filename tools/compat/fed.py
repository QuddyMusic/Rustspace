"""Сценарии федерации: пара инстансов X ↔ Y. Сценарий идёт дважды — по паре
оригинал ↔ оригинал и по паре Rust ↔ оригинал, — журналы ответов и событий
WebSocket сравниваются после нормализации идентификаторов, секретов и адресов."""
import sys, json, re, time, random, string, hmac, hashlib, uuid
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from lib import http, diff
from scn import Norm, Listener, TS_KEYS
import sqlite3

class Side:
    """Один инстанс пары."""
    def __init__(self, pair, role, port):
        self.pair, self.role, self.port = pair, role, port
        self.origin = f'http://localhost:{port}'
        self.tok = {}; self.uid = {}; self.ws = {}
    def user(self, name, listen=True):
        full = f'{name}{self.pair.tag}'
        s, b = http(self.port, 'POST', '/api/auth/register', {'username': full, 'password': 'password123'})
        if s == 409:
            s, b = http(self.port, 'POST', '/api/auth/login', {'username': full, 'password': 'password123'})
        assert s in (200, 201), (s, b)
        self.tok[name] = b['token']; self.uid[name] = b['user']['id']; self.pair.n.tok(b['user']['id'])
        if listen: self.ws[name] = Listener(self.port, b['token'])
        return b['user']
    def call(self, who, method, path, body=None, label=None, quiet=False):
        s, b = http(self.port, method, path, body, self.tok.get(who))
        if not quiet:
            self.pair.log.append((label or f'{self.role}:{who} {method} {self.pair.text(path)}', s, self.pair.val(b)))
        return s, (b if isinstance(b, (dict, list)) else {})
    def signed(self, method, path, body, secret, origin, label, nonce=True, raw=None, sign_as=None):
        """Подписанный запрос между инстансами; возвращает статус, заголовки и тело."""
        import http.client
        text = raw if raw is not None else json.dumps(body, separators=(',', ':'), ensure_ascii=False)
        ts = str(int(time.time() * 1000)); n = str(uuid.uuid4()) if nonce else None
        signed_text = sign_as if sign_as is not None else text
        payload = f'{ts}.{n}.{signed_text}' if n else f'{ts}.{signed_text}'
        h = {'Content-Type': 'application/json'}
        if secret is not None:
            h['X-Federation-Signature'] = 'sha256=' + hmac.new(secret.encode(), payload.encode(), hashlib.sha256).hexdigest()
            h['X-Federation-Origin'] = origin; h['X-Federation-Timestamp'] = ts
            if n: h['X-Federation-Nonce'] = n
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=20)
        conn.request(method, path, body=text.encode(), headers=h)
        r = conn.getresponse(); data = r.read(); got = {k.lower(): v for k, v in r.getheaders()}; conn.close()
        try: parsed = json.loads(data) if data else None
        except Exception: parsed = data.decode(errors='replace')
        self.pair.log.append((f'{self.role}:s2s {label}', r.status, self.pair.val(parsed)))
        return r.status, got, data
    def db(self, sql, args=()):
        pg = __import__('os').environ.get('BS_FED_PG')
        if pg and self.port in self.pair.rust_ports:
            return self.pg(pg, sql, args)
        c = sqlite3.connect(f'/tmp/bsfed{self.port}/bs.db', timeout=10)
        try: return c.execute(sql, args).fetchall()
        finally: c.close()
    def pg(self, url, sql, args):
        """Тот же запрос к базе PostgreSQL инстанса на Rust (через psql)."""
        import subprocess
        for a in args: sql = sql.replace('?', "'" + str(a).replace("'", "''") + "'", 1)
        # Логические выражения SQLite дают 0/1, как и здесь после приведения.
        sql = re.sub(r'(\w+ is (?:not )?null)', r'(\1)::int', sql)
        out = subprocess.run(['psql', f'{url}/bsfed{self.port}', '-AtF', '\x1f', '-c', sql], capture_output=True, text=True, check=True).stdout
        rows = []
        for line in out.splitlines():
            rows.append(tuple(None if v == '' else int(v) if re.fullmatch(r'-?\d+', v) else v for v in line.split('\x1f')))
        return rows
    def note(self, label, value):
        """Произвольное наблюдение (например, строка базы) в журнал сравнения."""
        self.pair.log.append((f'{self.role}:{label}', 0, self.pair.val(value)))

class Pair:
    def __init__(self, xport, yport, tag):
        self.tag = tag; self.n = Norm(); self.log = []
        self.x = Side(self, 'X', xport); self.y = Side(self, 'Y', yport)
        self.roles = {str(xport): 'X', str(yport): 'Y'}
        rust_y = bool(__import__('os').environ.get('BS_FED_RUST_Y'))
        self.rust_ports = {3113} | ({3114} if rust_y else set())
    def text(self, s):
        s = re.sub(r'(?:localhost|127\.0\.0\.1):(31\d\d)', lambda m: 'host-' + self.roles.get(m.group(1), m.group(1)), s)
        return self.n.text(s)
    def val(self, v, key=''):
        # Время последней синхронизации журнала мутаций ставит подтягивание
        # через /api/federation/sync — это шаг 6 переноса; до него не сравнивается.
        if key == 'lastSyncedAt': return '<sync>'
        if isinstance(v, dict): return {self.text(k): self.val(v[k], k) for k in sorted(v)}
        if isinstance(v, list): return [self.val(x, key) for x in v]
        if isinstance(v, str):
            if key in ('instanceId', 'deadEpoch', 'newEpoch', 'approvalToken', 'hmacSecret') and v: return self.n.tok(v)
            return self.text(v)
        if isinstance(v, (int, float)) and not isinstance(v, bool) and TS_KEYS.search(key) and v > 1_600_000_000_000: return '<ts>'
        return v
    def settle(self, t=0.6): time.sleep(t)
    def events(self):
        out = {}
        for side in (self.x, self.y):
            for name, l in side.ws.items():
                out[f'{side.role}:{name}'] = [self.val(e) for e in l.events]
        return out
    def close(self):
        for side in (self.x, self.y):
            for l in side.ws.values(): l.stop = True

def run(scenario, pairs=((3111, 3112), (3113, 3114)), show=False, unordered_events=True):
    tag = ''.join(random.choice(string.ascii_lowercase) for _ in range(5))
    res = []
    for xp, yp in pairs:
        p = Pair(xp, yp, tag); scenario(p); p.settle(1.0); res.append((p.log, p.events())); p.close()
    (la, ea), (lb, eb) = res
    bad = 0
    if len(la) != len(lb):
        bad += 1; print(f'✗ число шагов {len(la)} != {len(lb)}')
    for i, (x, y) in enumerate(zip(la, lb)):
        d = [] if x[1] == y[1] else [f'статус {x[1]} != {y[1]}  тело ориг: {json.dumps(x[2], ensure_ascii=False)[:300]}  rust: {json.dumps(y[2], ensure_ascii=False)[:300]}']
        d += diff(x[2], y[2])
        if d:
            bad += 1; print(f'✗ [{i}] {x[0]}'); [print('    ', l) for l in d[:12]]
        elif show: print(f'✓ [{i}] {x[0]} → {x[1]}  {json.dumps(x[2], ensure_ascii=False)[:160]}')
    for name in ea:
        a, b = ea[name], eb.get(name, [])
        key = lambda e: json.dumps(e, sort_keys=True, ensure_ascii=False)
        # Последствия переходов оригинал выполняет в фоне: порядок событий
        # разных переходов не определён, сравниваются наборы.
        sa, sb = (sorted(map(key, a)), sorted(map(key, b))) if unordered_events else (list(map(key, a)), list(map(key, b)))
        if sa != sb:
            bad += 1; print(f'✗ события {name}:')
            from collections import Counter
            ca, cb = Counter(sa), Counter(sb)
            for k in sorted(set(ca) | set(cb)):
                if ca[k] != cb[k]: print(f'     ориг ×{ca[k]}  rust ×{cb[k]}  {k[:220]}')
        elif show: print(f'✓ события {name}: {[e.get("type") for e in a]}')
    print(f'{scenario.__name__}: шагов {len(la)}, расхождений {bad}')
    return bad

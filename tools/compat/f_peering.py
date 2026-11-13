"""Пиринг: рукопожатие в обе стороны, проверки подписи, смена секрета,
одобрение и отказ, просьбы пользователей, сброс удалённого инстанса."""
import hmac, hashlib, json
from fed import run

INIT = '/api/federation/peer/initiate'
PEERS = '/api/federation/peers'

def peer_of(side, origin):
    _, b = side.call('root', 'GET', PEERS, quiet=True)
    for peer in b.get('peers', []):
        if peer['origin'] == origin: return peer
    return None

def secret_of(side, origin):
    rows = side.db('select hmac_secret from federation_peers where origin = ?', (origin,))
    return rows[0][0] if rows else None

def row_of(side, origin):
    rows = side.db('select status, status_reason, initiated_by, peer_instance_id is not null, approval_token is not null, '
                   'pending_hmac_secret is not null, probe_attempts from federation_peers where origin = ?', (origin,))
    return list(rows[0]) if rows else None

def forget(side, origin):
    """Отзывает и удаляет пира, если он есть."""
    peer = peer_of(side, origin)
    if not peer: return
    if peer['status'] not in ('revoked', 'rejected'):
        side.call('root', 'DELETE', f"{PEERS}/{peer['id']}", quiet=True)
    side.call('root', 'DELETE', f"{PEERS}/{peer['id']}/permanent", quiet=True)

def both(p, label):
    p.settle()
    p.x.call('root', 'GET', PEERS, label=f'X peers {label}'); p.y.call('root', 'GET', PEERS, label=f'Y peers {label}')
    p.x.note(f'row {label}', row_of(p.x, p.y.origin)); p.y.note(f'row {label}', row_of(p.y, p.x.origin))

def peering(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root'); x.user('al'); x.user('bo')

    # Доступ и проверка полей.
    x.call('al', 'GET', PEERS); x.call(None, 'GET', PEERS); x.call('root', 'GET', PEERS)
    for body in [{}, {'remoteOrigin': 5}, {'remoteOrigin': ''}, {'remoteOrigin': 'ftp://chat.example'},
                 {'remoteOrigin': 'http://chat.example'}, {'remoteOrigin': x.origin}, {'remoteOrigin': x.origin + '/path?q=1'}]:
        x.call('root', 'POST', INIT, body)
    x.call('al', 'POST', INIT, {'remoteOrigin': y.origin})
    x.call('root', 'POST', INIT, {'remoteOrigin': 'http://localhost:3199'}, label='X initiate: никого нет')
    x.call('root', 'GET', PEERS, label='X peers после неудачи')

    # Рукопожатие X → Y: Y принимает сам.
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin + '/'}, label='X initiate Y')
    both(p, 'после рукопожатия')
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate Y повторно')
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X при действующем пиринге')

    # Подписанные запросы к X от имени Y.
    secret = secret_of(x, y.origin)
    E = '/api/federation/epoch'
    x.signed('POST', E, {}, None, None, 'epoch без заголовков')
    x.signed('POST', E, {}, secret, 'http://localhost:3198', 'epoch от незнакомца')
    x.signed('POST', E, {}, 'f' * 64, y.origin, 'epoch с чужим секретом')
    status, headers, data = x.signed('POST', E, {}, secret, y.origin, 'epoch')
    ts, nonce = headers.get('x-federation-timestamp', ''), headers.get('x-federation-nonce', '')
    want = hmac.new(secret.encode(), f'{ts}.{nonce}.{data.decode()}'.encode(), hashlib.sha256).hexdigest()
    x.note('epoch: ответ подписан', [status, headers.get('x-federation-signature') == 'sha256=' + want, 'x-federation-origin' in headers])
    x.signed('POST', E, {}, secret, y.origin, 'epoch без nonce', nonce=False)
    x.signed('POST', E, None, secret, y.origin, 'epoch: тело с пробелами', raw=' { } ', sign_as='{}')
    x.signed('POST', E, None, secret, y.origin, 'epoch: числа, порядок ключей, экранирование',
             raw='{"b":1.0,"10":2e2,"a":"\\u0041\\/","2":[1.50,-0],"b":true}', sign_as='{"2":[1.5,0],"10":200,"b":true,"a":"A/"}')
    x.signed('POST', E, None, secret, y.origin, 'epoch: подпись по сырым байтам не годится', raw='{ "a": 1 }')
    x.signed('POST', E, None, secret, y.origin, 'epoch: не JSON', raw='{oops')
    R = '/api/federation/peer/rotate'
    x.signed('POST', R, {'newSecret': 'a' * 64}, None, None, 'rotate без заголовков')
    x.signed('POST', R, {'newSecret': 'a' * 64}, secret, 'http://localhost:3198', 'rotate от незнакомца')
    x.signed('POST', R, {'newSecret': 'a' * 64}, 'f' * 64, y.origin, 'rotate с чужим секретом')
    for bad in ['abc', 'A' * 64, 'g' * 64, 5, None]:
        x.signed('POST', R, {'newSecret': bad}, secret, y.origin, f'rotate: негодный секрет {str(bad)[:6]}')
    D = '/api/federation/peer/denied'
    x.signed('POST', D, {'origin': y.origin, 'reason': 'denied_by_admin'}, None, None, 'denied без заголовков')
    x.signed('POST', D, {'origin': y.origin, 'reason': 'denied_by_admin'}, secret, 'http://localhost:3198', 'denied от незнакомца')
    x.signed('POST', D, {'origin': y.origin, 'reason': 'denied_by_admin'}, secret, y.origin, 'denied при действующем пиринге')

    # Смена секрета и настройки пира.
    px, py = peer_of(x, y.origin), peer_of(y, x.origin)
    x.call('al', 'POST', f"{PEERS}/{px['id']}/rotate")
    x.call('root', 'POST', f"{PEERS}/000/rotate")
    x.call('root', 'POST', f"{PEERS}/{px['id']}/rotate", label='X rotate')
    x.call('root', 'POST', f"{PEERS}/{px['id']}/rotate", label='X rotate повторно')
    y.call('root', 'POST', f"{PEERS}/{py['id']}/rotate", label='Y rotate во время смены')
    both(p, 'во время смены секрета')
    # Во время смены годятся оба секрета.
    x.signed('POST', E, {}, secret, y.origin, 'epoch старым секретом во время смены')
    pending = x.db('select pending_hmac_secret from federation_peers where origin = ?', (y.origin,))[0][0]
    x.signed('POST', E, {}, pending, y.origin, 'epoch новым секретом во время смены')
    for body in [{}, {'autoRotateIntervalDays': 0}, {'autoRotateIntervalDays': 366}, {'autoRotateIntervalDays': 1.5},
                 {'autoRotateIntervalDays': 'x'}, {'autoRotateIntervalDays': '30'}, {'autoRotateIntervalDays': 7}]:
        x.call('root', 'PATCH', f"{PEERS}/{px['id']}", body)
    x.call('root', 'PATCH', f"{PEERS}/000", {'autoRotateIntervalDays': 7})
    x.call('root', 'POST', f"{PEERS}/{px['id']}/recheck"); x.call('root', 'POST', f"{PEERS}/{px['id']}/reset")
    x.call('root', 'DELETE', f"{PEERS}/{px['id']}/permanent"); x.call('root', 'POST', f"{PEERS}/000/recheck")

    # Отзыв: X отзывает Y; Y пробует заново.
    x.call('root', 'DELETE', f"{PEERS}/000"); x.call('al', 'DELETE', f"{PEERS}/{px['id']}")
    x.call('root', 'DELETE', f"{PEERS}/{px['id']}", label='X отзывает Y')
    x.signed('POST', E, {}, secret, y.origin, 'epoch отозванному')
    forget(y, x.origin)
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X после отзыва')
    both(p, 'после отказа отозванному')
    x.call('root', 'DELETE', f"{PEERS}/{px['id']}/permanent", label='X удаляет отозванного')
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X снова')
    both(p, 'после пиринга от Y')

    # Сброс удалённого инстанса: Y «приходит» с другой эпохой.
    A = '/api/federation/peer/accept'
    for body in [{}, {'sourceOrigin': y.origin}, {'sourceOrigin': 'http://chat.example', 'hmacSecret': 'a' * 64}]:
        x.call(None, 'POST', A, body)
    x.call(None, 'POST', A, {'sourceOrigin': y.origin, 'hmacSecret': 'a' * 64, 'instanceId': 'other-epoch'}, label='accept с новой эпохой')
    both(p, 'после обнаружения сброса')
    x.call('al', 'GET', '/api/federation/reset-events'); x.call('root', 'GET', '/api/federation/reset-events')
    x.call(None, 'POST', A, {'sourceOrigin': y.origin, 'hmacSecret': 'a' * 64, 'instanceId': 'other-epoch'}, label='accept с новой эпохой повторно')
    for body in [{}, {'origin': 'http://localhost:3198'}, {'origin': y.origin}, {'origin': y.origin}]:
        x.call('root', 'POST', '/api/federation/reset-events/acknowledge', body)
    x.call('root', 'GET', '/api/federation/reset-events', label='reset-events после подтверждения')
    px = peer_of(x, y.origin)
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate при needs_attention')
    px = peer_of(x, y.origin)
    x.call('root', 'POST', f"{PEERS}/{px['id']}/reset", label='X reset') if px and px['status'] == 'needs_attention' else x.note('reset пропущен', px and px['status'])
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate: у Y остался прежний пиринг')
    both(p, 'после отложенного пиринга')
    forget(y, x.origin)
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate после сброса у Y')
    both(p, 'после повторного пиринга')
    x.call('root', 'GET', '/api/federation/reset-events', label='reset-events после повторного пиринга')

def approvals(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root'); x.user('al'); x.user('bo')
    forget(x, y.origin); forget(y, x.origin)
    AR = '/api/federation/approval-requests'
    x.call('root', 'PATCH', '/api/settings/instance', {'autoAcceptPeering': False}, quiet=True)

    # Y просит пиринг, X одобряет.
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X: нужна очередь')
    both(p, 'в очереди')
    x.call('al', 'GET', AR); _, listed = x.call('root', 'GET', AR)
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X: уже в очереди')
    x.call('root', 'POST', f'{AR}/000/approve'); x.call('al', 'POST', f"{AR}/{listed['requests'][0]['id']}/approve")
    x.call('root', 'POST', f"{AR}/{listed['requests'][0]['id']}/approve", label='X одобряет')
    both(p, 'после одобрения')
    x.call('root', 'GET', AR, label='очередь после одобрения')

    # Y просит пиринг, X отказывает.
    forget(x, y.origin); forget(y, x.origin)
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X: вторая просьба')
    _, listed = x.call('root', 'GET', AR, quiet=True)
    x.call('root', 'POST', f'{AR}/000/deny')
    x.call('root', 'POST', f"{AR}/{listed['requests'][0]['id']}/deny", label='X отказывает')
    both(p, 'после отказа')
    y.call('root', 'POST', INIT, {'remoteOrigin': x.origin}, label='Y initiate X после отказа')
    both(p, 'после повторной просьбы')
    # Администратор X передумал и начинает сам.
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate после своего отказа')
    both(p, 'после пиринга от X')

    # Просьбы пользователей при пиринге только с одобрения.
    forget(x, y.origin); forget(y, x.origin)
    EN = '/api/federation/peer/ensure'
    for body in [{}, {'remoteOrigin': 'http://chat.example'}, {'remoteOrigin': y.origin, 'reason': 'friend_request'}]:
        x.call('al', 'POST', EN, body)
    x.call(None, 'POST', EN, {'remoteOrigin': y.origin})
    x.call('al', 'POST', EN, {'remoteOrigin': y.origin}, label='al ensure Y')
    x.call('al', 'POST', EN, {'remoteOrigin': y.origin, 'reason': 'instance_connect'}, label='al ensure Y повторно')
    x.call('bo', 'POST', EN, {'remoteOrigin': y.origin}, label='bo ensure Y')
    x.call('bo', 'POST', EN, {'remoteOrigin': 'http://localhost:3199'}, label='bo ensure: никого нет')
    S = '/api/federation/peering-subscriptions'; N = '/api/federation/peering-notifications'
    _, al_subs = x.call('al', 'GET', S); _, bo_subs = x.call('bo', 'GET', S); x.call('root', 'GET', AR, label='очередь с просьбами')
    x.call('al', 'DELETE', f"{S}/{bo_subs['subscriptions'][0]['id']}"); x.call('al', 'DELETE', f'{S}/000')
    nobody = [s for s in bo_subs['subscriptions'] if '3199' in s['peerOrigin']][0]
    x.call('bo', 'DELETE', f"{S}/{nobody['id']}", label='bo отзывает просьбу')
    x.call('root', 'GET', AR, label='очередь без отозванной просьбы')
    x.call('bo', 'POST', EN, {'remoteOrigin': 'http://localhost:3199'}, label='bo ensure: никого нет, снова')
    _, listed = x.call('root', 'GET', AR, quiet=True)
    to_y = [r for r in listed['requests'] if r['origin'] == y.origin][0]; to_nobody = [r for r in listed['requests'] if '3199' in r['origin']][0]
    x.call('root', 'POST', f"{AR}/{to_nobody['id']}/approve", label='X одобряет просьбу к отсутствующему')
    x.call('root', 'POST', f"{AR}/{to_nobody['id']}/deny", label='X отказывает в просьбе')
    x.call('root', 'POST', f"{AR}/{to_y['id']}/approve", label='X одобряет просьбу к Y')
    both(p, 'после одобрения просьбы')
    x.call('root', 'GET', AR, label='очередь пуста')
    x.call('al', 'GET', S, label='al подписки после одобрения')
    _, al_n = x.call('al', 'GET', N); _, bo_n = x.call('bo', 'GET', N + '?unread=1')
    x.call('al', 'POST', f"{N}/{bo_n['notifications'][0]['id']}/read"); x.call('al', 'POST', f'{N}/000/read')
    x.call('al', 'POST', f"{N}/{al_n['notifications'][0]['id']}/read"); x.call('al', 'GET', N + '?unread=1'); x.call('al', 'GET', N)
    x.call('bo', 'POST', f'{N}/read-all'); x.call('bo', 'POST', f'{N}/read-all'); x.call('bo', 'GET', N)
    x.call('al', 'POST', EN, {'remoteOrigin': y.origin}, label='al ensure при действующем пиринге')

    # Без одобрения: просьба пользователя сразу ведёт к рукопожатию.
    x.call('root', 'PATCH', '/api/settings/instance', {'autoAcceptPeering': True}, quiet=True)
    forget(x, y.origin); forget(y, x.origin)
    x.call('al', 'POST', EN, {'remoteOrigin': y.origin}, label='al ensure Y без одобрения')
    both(p, 'после ensure')
    # Y принимает только с одобрения: просьба ждёт на той стороне.
    forget(x, y.origin); forget(y, x.origin)
    y.call('root', 'PATCH', '/api/settings/instance', {'autoAcceptPeering': False}, quiet=True)
    x.user('cy')  # у al и bo исчерпан предел просьб (3 за 15 минут)
    x.call('cy', 'POST', EN, {'remoteOrigin': y.origin}, label='cy ensure: Y требует одобрения')
    both(p, 'ждём одобрения Y')
    x.call('root', 'POST', INIT, {'remoteOrigin': y.origin}, label='X initiate при ожидании одобрения')
    _, listed = y.call('root', 'GET', AR, label='очередь Y')
    y.call('root', 'POST', f"{AR}/{listed['requests'][0]['id']}/approve", label='Y одобряет')
    both(p, 'после одобрения Y')
    y.call('root', 'PATCH', '/api/settings/instance', {'autoAcceptPeering': True}, quiet=True)

if __name__ == '__main__':
    import sys
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['peering', 'approvals']
    sys.exit(1 if sum(run(globals()[n], show=show) for n in names) else 0)

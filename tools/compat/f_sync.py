"""Догонялка после простоя: события, которые до получателя не дошли, он
забирает из журнала мутаций отправителя (`/api/federation/sync`).

Стенд поднимается с `BS_FED_TEST_ROUTES=1` (догонялка запускается служебным
маршрутом сценария) и с обычными паузами повторов (`FEDERATION_BACKOFF_DIVISOR=1`):
пока получатель недоступен, отправитель успевает одну попытку, после чего
его очередь к получателю стирается — события остаются только в журнале.

`pull`: X (Rust или оригинал) забирает события у Y-оригинала.
`serve`: Y-оригинал забирает события у X — так проверяется выдача журнала X.
`pages`: подписанные запросы к `/api/federation/sync` X напрямую.
"""
import json, sys, time
from fed import run
from lib import http
from f_outbox import seed_replica, befriend, set_down
from f_inbound import local_view, replicas, ledgers


def lose_outbox(p, side, other):
    """`other` недоступен: события `side` копятся, затем очередь стирается."""
    time.sleep(1.5)
    side.exec('DELETE FROM federation_outbox')
    time.sleep(0.3)
    set_down(other, False)
    time.sleep(0.5)


def resync(p, side, other, label, **extra):
    s, b = http(side.port, 'POST', '/api/admin/test/federation/resync', {'peerOrigin': other.origin, **extra})
    p.log.append((f'{side.role}: {label}', s, p.val(b)))
    time.sleep(0.5)
    return b


def cursors(side, label):
    rows = side.db('SELECT context_type, cursor_ts > 0, cursor_id IS NOT NULL, last_pulled_at IS NOT NULL '
                   'FROM federation_sync_cursors ORDER BY context_type')
    kept = side.db('SELECT context_type, event_type, last_reason, attempts FROM federation_sync_retry '
                   'WHERE first_failed_at >= ? ORDER BY event_ts, id', (side.pair.started,))
    side.note(f'курсоры {label}', {'cursors': [list(r) for r in rows], 'kept': [list(r) for r in kept]})


def pull(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy')
    y.user('bob'); y.user('dan', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    time.sleep(1.5)
    al = seed_replica(y, x, 'al'); cy = seed_replica(y, x, 'cy')
    befriend(y, y.uid['bob'], al); befriend(y, y.uid['bob'], cy); befriend(y, y.uid['dan'], al)
    befriend(y, y.uid['bob'], y.uid['dan'])
    # X недоступен: всё, что делает Y, остаётся только в журнале Y.
    set_down(x, True)
    time.sleep(0.3)
    _, dm = y.call('bob', 'POST', '/api/dm', {'userId': al}, quiet=True)
    channel = dm.get('id')
    _, m1 = y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': f'привет <@{al}>'}, label='bob пишет')
    _, m2 = y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': 'второе', 'replyToId': m1.get('id')}, label='bob отвечает')
    y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': 'третье'}, label='bob пишет ещё')
    y.call('bob', 'PATCH', f"/api/dm/messages/{m1.get('id')}", {'content': 'привет, исправлено'}, label='bob правит')
    y.ws['bob'].send({'type': 'reaction_add', 'messageId': m1.get('id'), 'emoji': '👍'})
    y.ws['bob'].send({'type': 'reaction_add', 'messageId': m2.get('id'), 'emoji': '🔥'})
    time.sleep(0.4)
    y.ws['bob'].send({'type': 'reaction_remove', 'messageId': m2.get('id'), 'emoji': '🔥'})
    time.sleep(0.4)
    y.call('bob', 'DELETE', f"/api/dm/messages/{m2.get('id')}", label='bob удаляет')
    _, group = y.call('bob', 'POST', '/api/dm/group', {'users': [{'id': al}, {'id': y.uid['dan']}]}, label='bob создаёт группу')
    gid = group.get('id')
    y.call('bob', 'PATCH', f'/api/dm/{gid}', {'name': 'Команда'}, label='bob называет группу')
    y.call('bob', 'POST', f'/api/dm/{gid}/members', {'userId': cy}, label='bob добавляет cy')
    y.call('bob', 'POST', f'/api/dm/{gid}/messages', {'content': 'всем'}, label='bob пишет в группу')
    y.call('bob', 'DELETE', f"/api/dm/{gid}/members/{y.uid['dan']}", label='bob исключает dan')
    y.call('bob', 'PATCH', '/api/users/@me', {'displayName': 'Боб', 'bio': 'с Y'}, label='bob меняет профиль')
    y.call('dan', 'DELETE', f'/api/social/friends/{al}', label='dan удаляет al')
    lose_outbox(p, y, x)
    resync(p, x, y, 'догонялка у Y')
    local_view(x, 'al', 'после догонялки')
    local_view(x, 'cy', 'после догонялки')
    replicas(x, 'после догонялки')
    ledgers(x, 'после догонялки')
    cursors(x, 'после догонялки')
    resync(p, x, y, 'догонялка повторно')
    resync(p, x, y, 'догонялка: только профили', contexts=['profile'])
    cursors(x, 'в конце')
    s, b = http(x.port, 'GET', '/api/federation/peers', None, x.tok['root'])
    p.log.append(('X: пиры в конце', s, p.val(b)))


def serve(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy', listen=False)
    y.user('bob'); y.user('dan', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    time.sleep(1.5)
    bob = seed_replica(x, y, 'bob'); dan = seed_replica(x, y, 'dan')
    befriend(x, x.uid['al'], bob); befriend(x, x.uid['al'], dan); befriend(x, x.uid['cy'], bob)
    set_down(y, True)
    time.sleep(0.3)
    _, dm = x.call('al', 'POST', '/api/dm', {'userId': bob}, quiet=True)
    channel = dm.get('id')
    _, m1 = x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': f'привет <@{bob}>'}, label='al пишет')
    x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': 'второе', 'replyToId': m1.get('id')}, label='al отвечает')
    x.call('al', 'PATCH', f"/api/dm/messages/{m1.get('id')}", {'content': 'исправлено'}, label='al правит')
    x.ws['al'].send({'type': 'reaction_add', 'messageId': m1.get('id'), 'emoji': '👍'})
    time.sleep(0.4)
    x.ws['al'].send({'type': 'channel_ack', 'channelId': channel, 'messageId': m1.get('id')})
    time.sleep(0.4)
    _, group = x.call('al', 'POST', '/api/dm/group', {'users': [{'id': bob}, {'id': dan}]}, label='al создаёт группу')
    gid = group.get('id')
    x.call('al', 'PATCH', f'/api/dm/{gid}', {'name': 'Группа X'}, label='al называет группу')
    x.call('al', 'POST', f'/api/dm/{gid}/messages', {'content': 'всем'}, label='al пишет в группу')
    x.call('al', 'DELETE', f'/api/dm/{gid}/members/{dan}', label='al исключает dan')
    x.call('al', 'PATCH', '/api/users/@me', {'displayName': 'Ал', 'bio': 'с X'}, label='al меняет профиль')
    x.call('cy', 'DELETE', f'/api/social/friends/{bob}', label='cy удаляет bob')
    lose_outbox(p, x, y)
    resync(p, y, x, 'Y забирает у X')
    _, channels = y.call('bob', 'GET', '/api/dm', quiet=True)
    view = []
    for c in sorted(channels if isinstance(channels, list) else [], key=lambda c: c.get('createdAt', 0)):
        _, messages = y.call('bob', 'GET', f"/api/dm/{c['id']}/messages?limit=50", quiet=True)
        items = messages if isinstance(messages, list) else messages.get('messages', [])
        view.append({'channel': c, 'messages': sorted(items, key=lambda m: (m.get('createdAt', 0), m.get('content') or ''))})
    y.note('bob видит после догонялки', view)
    rows = y.db('SELECT username, display_name, bio FROM users WHERE username LIKE ? ORDER BY username', (f'al{p.tag}@%',))
    y.note('копия al', [list(r) for r in rows])
    resync(p, y, x, 'Y забирает повторно')


def pages(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root'); x.user('al')
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    time.sleep(1.5)
    y.user('bob', listen=False)
    bob = seed_replica(x, y, 'bob')
    befriend(x, x.uid['al'], bob)
    _, dm = x.call('al', 'POST', '/api/dm', {'userId': bob}, quiet=True)
    channel = dm.get('id')
    for i in range(7):
        x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': f'сообщение {i}'}, quiet=True)
    x.call('al', 'PATCH', '/api/users/@me', {'bio': 'страницы'}, quiet=True)
    time.sleep(1.0)
    secret = x.db('SELECT hmac_secret FROM federation_peers')
    secret = secret[0][0] if secret else 'none'
    ask = lambda body, label, **kw: x.signed('POST', '/api/federation/sync', body, secret, y.origin, label, **kw)
    since = p.started
    ask({}, 'без времени')
    ask({'sinceTimestamp': -1}, 'отрицательное время')
    ask({'sinceTimestamp': '5'}, 'время строкой')
    ask(None, 'тело null', raw='null')
    ask({'sinceTimestamp': since}, 'переписки')
    _, _, raw = ask({'sinceTimestamp': since, 'limit': 3}, 'по 3')
    page = json.loads(raw) if raw else {}
    ask({'sinceTimestamp': page.get('checkpoint', 0), 'afterId': page.get('checkpointId', ''), 'limit': 3}, 'следующие 3')
    ask({'sinceTimestamp': since, 'limit': 0.5}, 'предел меньше 1')
    ask({'sinceTimestamp': since, 'limit': 'x'}, 'предел не число')
    ask({'sinceTimestamp': since, 'dmChannelId': channel}, 'одна переписка')
    ask({'sinceTimestamp': since, 'federatedId': 'nope'}, 'неизвестная переписка')
    ask({'sinceTimestamp': since, 'contextType': 'profile'}, 'профили')
    ask({'sinceTimestamp': since, 'contextType': 'friend'}, 'дружба')
    ask({'sinceTimestamp': 9_000_000_000_000}, 'из будущего')
    x.signed('POST', '/api/federation/sync', {'sinceTimestamp': 0}, secret, 'https://z.test', 'от неизвестного')
    for label, body in [('служебный пир: не http', {'origin': 'ftp://z'}),
                        ('служебный пир: статус', {'origin': 'https://z.test', 'hmacSecret': 'k' * 40, 'status': 'odd'})]:
        s, b = http(x.port, 'POST', '/api/admin/test/seed-peer', body)
        p.log.append((f'X: {label}', s, p.val(b)))
    for label, body in [('догонялка без пира', {}), ('догонялка с неизвестным', {'peerOrigin': 'https://nope.test'})]:
        s, b = http(x.port, 'POST', '/api/admin/test/federation/resync', body)
        p.log.append((f'X: {label}', s, p.val(b)))


if __name__ == '__main__':
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['pull', 'serve', 'pages']
    sys.exit(1 if sum(run(globals()[n], show=show, ignore_events=('dm_typing_start', 'dm_typing_stop'))
                      for n in names) else 0)

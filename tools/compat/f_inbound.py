"""Приём событий: изменения на Y (сообщения, правки, реакции, удаления,
вложения, группы, дружба, профиль, присутствие) приходят на X.

В паре «ориг» X — оригинал, в паре «rust» X — сервер на Rust; Y в обеих —
оригинал (с BS_FED_RUST_Y=1 — тоже Rust). Сравниваются ответы, что видит
получатель на X (переписки, сообщения, копии пользователей), строки базы X
(журнал применённых событий, часы, очередь файлов) и события WebSocket.

Копия пользователя X на Y создаётся прямо в базе Y — так, как её создаёт
оригинал; поиск `user@домен` проверяет `f_lookup.py`.
"""
import base64, http.client, json, os, sys, time
from fed import run
from f_outbox import seed_replica, befriend, set_down

T = {'Tus-Resumable': '1.0.0'}


def upload(side, who, name, data, kind='text/plain'):
    """Загрузка файла по tus; возвращает вложение."""
    b64 = lambda s: base64.b64encode(s.encode()).decode()
    auth = {'Authorization': 'Bearer ' + side.tok[who]}
    conn = http.client.HTTPConnection('127.0.0.1', side.port, timeout=20)
    conn.request('POST', '/api/files', headers={**T, **auth, 'Upload-Length': str(len(data)),
                 'Upload-Metadata': f'filename {b64(name)},filetype {b64(kind)}'})
    r = conn.getresponse(); r.read(); loc = r.getheader('location'); conn.close()
    conn = http.client.HTTPConnection('127.0.0.1', side.port, timeout=20)
    conn.request('PATCH', loc, body=data, headers={**T, **auth, 'Upload-Offset': '0',
                 'Content-Type': 'application/offset+octet-stream'})
    r = conn.getresponse(); body = r.read(); conn.close()
    return json.loads(body) if body else {}


def wait_quiet(p, t=6.0):
    """Ждёт, пока очереди исходящих X и Y опустеют."""
    deadline = time.time() + t
    time.sleep(0.2)
    while time.time() < deadline:
        if not p.y.db('SELECT 1 FROM federation_outbox LIMIT 1') and not p.x.db('SELECT 1 FROM federation_outbox LIMIT 1'):
            break
        time.sleep(0.2)
    time.sleep(0.8)


def local_view(x, who, label):
    """Что видит получатель на X: переписки и сообщения в них."""
    _, channels = x.call(who, 'GET', '/api/dm', quiet=True)
    view = []
    for channel in sorted(channels if isinstance(channels, list) else [], key=lambda c: c.get('createdAt', 0)):
        _, messages = x.call(who, 'GET', f"/api/dm/{channel['id']}/messages?limit=50", quiet=True)
        items = messages if isinstance(messages, list) else messages.get('messages', [])
        view.append({'channel': channel, 'messages': sorted(items, key=lambda m: (m.get('createdAt', 0), m.get('content') or ''))})
    x.note(f'{who} видит {label}', view)


def replicas(x, label):
    rows = x.db('SELECT username, display_name, avatar, banner, bio, home_instance, '
                'password_hash = \'!federation-replicated\', status, is_deleted FROM users WHERE home_instance IS NOT NULL AND (username LIKE ? OR username LIKE ?) '
                'ORDER BY username', (f'%{x.pair.tag}%', '!deleted:%'))
    rows = [r for r in rows if x.pair.tag in r[0] or r[0].startswith('!deleted:') and r[0][9:] in x.pair.n.ids]
    x.note(f'копии {label}', [list(r) for r in rows])


def ledgers(x, label):
    since = x.pair.started
    applied = x.db('SELECT source_origin, event_key FROM federation_applied_events WHERE applied_at >= ? ORDER BY event_key', (since,))
    clocks = x.db('SELECT subject_key FROM federation_subject_clocks WHERE recorded_at >= ? ORDER BY subject_key', (since,))
    files = x.db('SELECT peer_origin, source_url, original_name, mimetype, size, status, rejection_reason '
                 'FROM federation_file_queue WHERE created_at >= ? ORDER BY created_at, id', (since,))
    x.note(f'журналы {label}', {'applied': [list(r) for r in applied], 'clocks': [r[0] for r in clocks],
                                'files': [list(r) for r in files]})


def inbound(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy')
    y.user('bob'); y.user('dan', listen=False); y.user('fay', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    al = seed_replica(y, x, 'al'); cy = seed_replica(y, x, 'cy')
    befriend(y, y.uid['bob'], al); befriend(y, y.uid['bob'], cy); befriend(y, y.uid['dan'], al)
    befriend(y, y.uid['bob'], y.uid['dan'])

    # Личная переписка: bob пишет al, копия bob появляется на X.
    _, dm = y.call('bob', 'POST', '/api/dm', {'userId': al}, label='bob открывает переписку с al')
    channel = dm.get('id')
    _, m1 = y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': f'привет <@{al}> и <@{cy}>'}, label='bob пишет')
    wait_quiet(p)
    local_view(x, 'al', 'первое сообщение')
    replicas(x, 'после первого сообщения')

    # Правка, ответ, реакции, вложение, удаление.
    y.call('bob', 'PATCH', f"/api/dm/messages/{m1.get('id')}", {'content': 'привет, исправлено'}, label='bob правит')
    _, m2 = y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': 'ответ', 'replyToId': m1.get('id')}, label='bob отвечает')
    y.ws['bob'].send({'type': 'reaction_add', 'messageId': m1.get('id'), 'emoji': '👍'})
    y.ws['bob'].send({'type': 'reaction_add', 'messageId': m2.get('id'), 'emoji': '🔥'})
    # Снятие реакции ждёт доставки постановки: иначе очередь Y может
    # свернуть пару в зависимости от момента отправки.
    p.settle(0.5)
    wait_quiet(p)
    y.ws['bob'].send({'type': 'reaction_remove', 'messageId': m2.get('id'), 'emoji': '🔥'})
    att = upload(y, 'bob', 'заметка.txt', 'содержимое'.encode())
    y.call('bob', 'POST', f'/api/dm/{channel}/messages', {'content': 'файл', 'attachments': [att.get('id')]}, label='bob шлёт файл')
    wait_quiet(p)
    local_view(x, 'al', 'правки, реакций и файла')
    y.call('bob', 'DELETE', f"/api/dm/messages/{m2.get('id')}", label='bob удаляет ответ')
    wait_quiet(p)
    # al отвечает со своей стороны и ставит реакцию на сообщение bob.
    x_channel = x.db('SELECT dm_channel_id FROM dm_messages WHERE source_message_id = ?', (m1.get('id'),))
    x_channel = x_channel[0][0] if x_channel else 'none'
    x_m1 = x.db('SELECT id FROM dm_messages WHERE source_message_id = ?', (m1.get('id'),))
    x.call('al', 'POST', f'/api/dm/{x_channel}/messages', {'content': 'и тебе'}, label='al отвечает на X')
    if x_m1:
        x.ws['al'].send({'type': 'reaction_add', 'messageId': x_m1[0][0], 'emoji': '🙂'})
    wait_quiet(p)
    # Позиция чтения и закрытие у bob.
    y.ws['bob'].send({'type': 'channel_ack', 'channelId': channel, 'messageId': m1.get('id')})
    p.settle(0.4)
    y.call('bob', 'DELETE', f'/api/dm/{channel}', label='bob закрывает переписку')
    wait_quiet(p)
    local_view(x, 'al', 'удаления и закрытия')

    # Группа: создание, название, добавление, исключение, передача, выход.
    _, group = y.call('bob', 'POST', '/api/dm/group', {'users': [{'id': al}, {'id': y.uid['dan']}]}, label='bob создаёт группу')
    gid = group.get('id')
    wait_quiet(p)
    local_view(x, 'al', 'создания группы')
    y.call('bob', 'PATCH', f'/api/dm/{gid}', {'name': 'Команда'}, label='bob называет группу')
    y.call('bob', 'POST', f'/api/dm/{gid}/members', {'userId': cy}, label='bob добавляет cy')
    wait_quiet(p)
    y.call('bob', 'POST', f'/api/dm/{gid}/messages', {'content': 'всем привет'}, label='bob пишет в группу')
    wait_quiet(p)
    local_view(x, 'cy', 'добавления')
    y.call('bob', 'DELETE', f"/api/dm/{gid}/members/{y.uid['dan']}", label='bob исключает dan')
    wait_quiet(p)
    y.call('bob', 'POST', f'/api/dm/{gid}/transfer', {'newOwnerId': al}, label='bob передаёт группу al')
    wait_quiet(p)
    local_view(x, 'al', 'передачи')
    y.call('bob', 'DELETE', f'/api/dm/{gid}/members/{y.uid["bob"]}', label='bob выходит из группы')
    wait_quiet(p)
    local_view(x, 'cy', 'выхода bob')

    # Дружба: удаление друга на Y.
    y.call('dan', 'DELETE', f'/api/social/friends/{al}', label='dan удаляет al из друзей')
    wait_quiet(p)

    # Профиль и присутствие.
    y.call('bob', 'PATCH', '/api/users/@me', {'displayName': 'Боб', 'bio': 'с Y'}, label='bob меняет профиль')
    y.ws['bob'].send({'type': 'presence_update', 'status': 'dnd'})
    p.settle(0.4)
    wait_quiet(p)
    replicas(x, 'после профиля')
    ledgers(x, 'в конце')


def relay_checks(p):
    """Прямые запросы к /api/federation/relay: подпись, источник, формат,
    отказы по отдельным событиям."""
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root'); x.user('al')
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    secret = x.db('SELECT hmac_secret FROM federation_peers')
    secret = secret[0][0] if secret else 'none'
    origin = y.origin
    send = lambda body, label, **kw: x.signed('POST', '/api/federation/relay', body, secret, origin, label, **kw)
    send({'version': 1, 'sourceInstance': origin, 'events': []}, 'пустая пачка')
    send({'version': 2, 'sourceInstance': origin, 'events': []}, 'неверная версия')
    send({'version': 1, 'events': []}, 'без источника')
    send({'version': 1, 'sourceInstance': 'https://z.test', 'events': []}, 'чужой источник')
    send({'version': 1, 'sourceInstance': origin, 'events': [{'eventType': 'x', 'messageId': str(i)} for i in range(51)]}, 'больше 50')
    x.signed('POST', '/api/federation/relay', {'version': 1, 'sourceInstance': origin, 'events': []}, 'f' * 64, origin, 'неверная подпись')
    x.signed('POST', '/api/federation/relay', {'version': 1, 'sourceInstance': origin, 'events': []}, secret, 'https://z.test', 'неизвестный пир')
    x.signed('POST', '/api/federation/relay', {'version': 1}, None, origin, 'без заголовков')
    al_id = x.uid['al']
    events = [
        {'eventType': 'nonsense', 'messageId': 'e1', 'timestamp': 1},
        {'eventType': 'create', 'messageId': 'e2', 'timestamp': 1},
        {'eventType': 'create', 'messageId': 'e3', 'timestamp': 1, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'x', 'createdAt': 1},
         'participants': [{'homeUserId': 'u1', 'homeInstance': origin}]},
        {'eventType': 'create', 'messageId': 'e4', 'timestamp': 1, 'message': {'homeUserId': 'u1', 'homeInstance': 'https://z.test', 'content': 'x', 'createdAt': 1},
         'participants': [{'homeUserId': 'u1', 'homeInstance': 'https://z.test'}, {'homeUserId': al_id, 'homeInstance': x.origin}]},
        {'eventType': 'create', 'messageId': 'e5', 'timestamp': 1, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'сист', 'type': 'system', 'createdAt': 1},
         'participants': [{'homeUserId': 'u1', 'homeInstance': origin}, {'homeUserId': al_id, 'homeInstance': x.origin}]},
        {'eventType': 'create', 'messageId': 'e6', 'timestamp': 1, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'прямо', 'createdAt': 1_700_000_000_000},
         'participants': [{'homeUserId': 'u1', 'homeInstance': origin, 'profile': {'username': 'ghost', 'displayName': 'Призрак', 'status': 'idle'}},
                          {'homeUserId': al_id, 'homeInstance': x.origin}]},
        {'eventType': 'create', 'messageId': 'e6', 'timestamp': 1, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'повтор', 'createdAt': 1},
         'participants': [{'homeUserId': 'u1', 'homeInstance': origin}, {'homeUserId': al_id, 'homeInstance': x.origin}]},
        {'eventType': 'update', 'messageId': 'e6', 'timestamp': 2, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'правка', 'editedAt': 1_700_000_000_500}},
        {'eventType': 'update', 'messageId': 'e6', 'timestamp': 2, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'старая', 'editedAt': 1_700_000_000_100}},
        {'eventType': 'reaction_add', 'messageId': 'e9', 'timestamp': 3, 'reaction': {'homeUserId': 'u1', 'homeInstance': origin, 'messageId': 'e6', 'messageHomeInstance': origin, 'emoji': '👀', 'createdAt': 1_700_000_000_600}},
        {'eventType': 'reaction_add', 'messageId': 'e10', 'timestamp': 3, 'reaction': {'homeUserId': 'u2', 'homeInstance': origin, 'messageId': 'e6', 'messageHomeInstance': origin, 'emoji': '👀'}},
        {'eventType': 'reaction_remove', 'messageId': 'e11', 'timestamp': 4, 'reaction': {'homeUserId': 'u1', 'homeInstance': origin, 'messageId': 'nope', 'emoji': '👀'}},
        {'eventType': 'delete', 'messageId': 'e12', 'timestamp': 5},
        {'eventType': 'create', 'messageId': 'e12', 'timestamp': 5, 'message': {'homeUserId': 'u1', 'homeInstance': origin, 'content': 'поздно', 'createdAt': 1},
         'participants': [{'homeUserId': 'u1', 'homeInstance': origin}, {'homeUserId': al_id, 'homeInstance': x.origin}]},
        {'eventType': 'presence_update', 'messageId': 'e13', 'timestamp': 6, 'presenceUpdate': {'homeUserId': 'u1', 'homeInstance': origin, 'status': 'dnd',
         'activities': [{'type': 'playing', 'name': 'Игра'}]}},
        {'eventType': 'presence_update', 'messageId': 'e14', 'timestamp': 6, 'presenceUpdate': {'homeUserId': 'u1', 'homeInstance': 'https://z.test', 'status': 'dnd'}},
        {'eventType': 'presence_update', 'messageId': 'e15', 'timestamp': 6, 'presenceUpdate': {'homeUserId': 'u1', 'homeInstance': origin, 'status': 'busy'}},
        {'eventType': 'profile_update', 'messageId': 'e16', 'timestamp': 7, 'profileUpdate': {'homeUserId': 'u1', 'homeInstance': origin, 'username': 'ghost',
         'displayName': 'Новый', 'avatar': 'a.webp', 'banner': None, 'avatarColor': 'red', 'bio': 'био', 'profileUpdatedAt': 1_700_000_001_000}},
        {'eventType': 'profile_update', 'messageId': 'e17', 'timestamp': 7, 'profileUpdate': {'homeUserId': 'u1', 'homeInstance': origin, 'username': 'ghost',
         'displayName': 'Старый', 'profileUpdatedAt': 1_700_000_000_000}},
        {'eventType': 'friend_request_create', 'messageId': 'e18', 'timestamp': 8, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}, 'fromProfile': {'username': 'ghost'}, 'createdAt': 1_700_000_002_000}},
        {'eventType': 'friend_request_create', 'messageId': 'e18', 'timestamp': 8, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}}},
        {'eventType': 'friend_request_cancel', 'messageId': 'e19', 'timestamp': 9, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}}},
        {'eventType': 'friend_request_create', 'messageId': 'e20', 'timestamp': 10, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}, 'createdAt': 1_700_000_003_000}},
        {'eventType': 'friend_add', 'messageId': 'e21', 'timestamp': 11, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}}},
        {'eventType': 'friend_remove', 'messageId': 'e22', 'timestamp': 12, 'friendship': {'from': {'homeUserId': 'u1', 'homeInstance': origin},
         'to': {'homeUserId': al_id, 'homeInstance': x.origin}}},
        {'eventType': 'dm_close', 'messageId': 'e23', 'timestamp': 13, 'federatedId': 'nope', 'dmCloseReopen': {'homeUserId': 'u1', 'homeInstance': origin}},
        {'eventType': 'read_state_update', 'messageId': 'e24', 'timestamp': 14, 'federatedId': 'nope',
         'readState': {'user': {'homeUserId': 'u1', 'homeInstance': origin}, 'messageRef': {'sourceInstance': origin, 'sourceMessageId': 'e6'}}},
        {'eventType': 'member_add', 'messageId': 'e25', 'timestamp': 15, 'federatedId': 'g1', 'membership': {'user': {'homeUserId': al_id, 'homeInstance': x.origin}}},
        {'eventType': 'group_metadata_update', 'messageId': 'e26', 'timestamp': 16, 'federatedId': 'g1', 'metadata': {'name': 'x', 'icon': None, 'metadataUpdatedAt': 5}},
        {'eventType': 'file_rejected', 'messageId': 'e27', 'timestamp': 17},
    ]
    # Идентификаторы событий и пользователей Y — свои в каждом прогоне: стенд
    # не пересоздаётся между запусками.
    import re
    events = json.loads(re.sub(r'"(u[12]|e\d+|ghost|nope|g1)"', lambda m: f'"{p.tag}{m.group(1)}"', json.dumps(events)))
    for i in range(len(events)):
        p.n.tok(events[i]['messageId'])
    send({'version': 1, 'sourceInstance': origin, 'events': events}, 'пачка событий')
    send({'version': 1, 'sourceInstance': origin, 'capabilities': ['attribution_unproven'],
          'events': [{'eventType': 'create', 'messageId': 'e30', 'timestamp': 1,
                      'message': {'homeUserId': p.tag + '424242', 'homeInstance': x.origin, 'content': 'x', 'createdAt': 1},
                      'participants': [{'homeUserId': al_id, 'homeInstance': x.origin}, {'homeUserId': p.tag + 'u1', 'homeInstance': origin}]}]},
         'наш пользователь без подтверждения')
    send({'version': 1, 'sourceInstance': origin, 'events': [{'eventType': 'create', 'messageId': 'e31', 'timestamp': 1,
          'message': {'homeUserId': al_id, 'homeInstance': x.origin, 'content': 'x', 'createdAt': 1},
          'participants': [{'homeUserId': al_id, 'homeInstance': x.origin}, {'homeUserId': p.tag + 'u1', 'homeInstance': origin}]}]},
         'наш пользователь, старый отправитель')
    local_view(x, 'al', 'после прямых событий')
    replicas(x, 'после прямых событий')
    ledgers(x, 'после прямых событий')
    x.signed('DELETE', '/api/federation/identity', {'homeUserId': p.tag + 'u1', 'homeInstance': origin, 'mode': 'bad'}, secret, origin, 'удаление: неверный режим')
    x.signed('DELETE', '/api/federation/identity', {'homeUserId': al_id, 'homeInstance': origin, 'mode': 'soft'}, secret, origin, 'удаление: нашего пользователя')
    x.signed('DELETE', '/api/federation/identity', {'homeUserId': p.tag + 'u1', 'homeInstance': origin, 'mode': 'soft'}, secret, origin, 'удаление копии')
    x.signed('DELETE', '/api/federation/identity', {'homeUserId': p.tag + 'u1', 'homeInstance': origin, 'mode': 'soft'}, secret, origin, 'удаление повторно')
    replicas(x, 'после удаления')
    x.call('root', 'GET', '/api/federation/peers', label='пиры X в конце')


if __name__ == '__main__':
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['inbound', 'relay_checks']
    # Печатание и звонки между инстансами — шаг 7 переноса.
    sys.exit(1 if sum(run(globals()[n], show=show, ignore_events=('dm_typing_start', 'dm_typing_stop'))
                      for n in names) else 0)

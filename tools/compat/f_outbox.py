"""Очередь исходящих и доставка: изменения на X (сообщения, правки, реакции,
удаления, группы, дружба, профиль, присутствие) уходят на Y.

Сравниваются ответы X, очередь исходящих X (что поставлено и что осталось
после доставки), полученное на Y (переписки и сообщения у получателя, его
события WebSocket) и строки журнала мутаций.

Копии пользователей Y на X создаются прямо в базе X — так, как их создаёт
оригинал (`createReplicatedUser`): поиск `user@домен` переносится позже.
"""
import json, sqlite3, time, random, string, os
from fed import run

SNOWFLAKE_EPOCH = 1_704_067_200_000


def snowflake():
    ms = int(time.time() * 1000) - SNOWFLAKE_EPOCH
    return str((ms << 22) | random.getrandbits(22))


def seed_replica(side, remote, name):
    """Копия пользователя `name` инстанса `remote` в базе `side`."""
    uid = snowflake()
    home_user = remote.uid[name]
    username = f'{name}{side.pair.tag}@{remote.host}'
    side.exec('INSERT INTO users (id, username, display_name, password_hash, status, is_admin, home_instance, '
              'home_user_id, created_at) VALUES (?, ?, NULL, ?, ?, 0, ?, ?, ?)',
              (uid, username, '!federation-replicated', 'offline', remote.host, home_user, int(time.time() * 1000)))
    side.uid[f'{name}@'] = uid
    side.pair.n.tok(uid)
    return uid


def befriend(side, a, b):
    now = int(time.time() * 1000)
    side.exec('INSERT INTO friends (user_id, friend_id, created_at) VALUES (?, ?, ?)', (a, b, now))


def outbox(side, label):
    """Очередь исходящих X: вид, контекст, содержимое (разобранное). Число
    попыток и отметка отправки зависят от времени и здесь не сравниваются."""
    rows = side.db('SELECT event_type, context_type, entity_id, queue_key, payload '
                   'FROM federation_outbox ORDER BY created_at, id')
    shown = []
    for kind, ctype, entity, key, payload in rows:
        shown.append({'event': kind, 'context': ctype, 'entity': entity, 'key': key, 'payload': json.loads(payload)})
    side.note(f'outbox {label}', shown)


def hold(p):
    """Y временно недоступен: события копятся в очереди X, пока их сравнивают."""
    set_down(p.y, True)
    time.sleep(0.3)


def mutation_log(side, label):
    rows = side.db('SELECT entity_id, context_type, mutation_type, payload FROM federation_mutation_log ORDER BY mutated_at, id')
    side.note(f'mutation log {label}', [[e, c, m, json.loads(p) if p else None] for e, c, m, p in rows])


def remote_view(y, who, label):
    """Что видит получатель на Y: переписки и сообщения в них."""
    _, channels = y.call(who, 'GET', '/api/dm', quiet=True)
    view = []
    for channel in sorted(channels if isinstance(channels, list) else [], key=lambda c: c.get('createdAt', 0)):
        _, messages = y.call(who, 'GET', f"/api/dm/{channel['id']}/messages?limit=50", quiet=True)
        items = messages if isinstance(messages, list) else messages.get('messages', [])
        view.append({'channel': channel, 'messages': sorted(items, key=lambda m: (m.get('createdAt', 0), m.get('content') or ''))})
    y.note(f'{who} видит {label}', view)


def deliver(p, t=4.0):
    """Возвращает Y и ждёт, пока очередь X опустеет (или выйдет время)."""
    set_down(p.y, False)
    deadline = time.time() + t
    while time.time() < deadline:
        if not p.x.db('SELECT 1 FROM federation_outbox WHERE next_retry_at <= ? LIMIT 1', (int(time.time() * 1000) + 100,)):
            break
        time.sleep(0.2)
    time.sleep(0.6)


def delivery(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy', listen=False)
    # Пользователи Y не подключаются: их присутствие ушло бы на X, а приём
    # событий на X — следующий шаг переноса. Что получил Y, видно по REST.
    y.user('bob', listen=False); y.user('dan', listen=False)
    # Пиринг X ↔ Y.
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    y.user('fay', listen=False)
    bob = seed_replica(x, y, 'bob'); dan = seed_replica(x, y, 'dan'); fay = seed_replica(x, y, 'fay')
    befriend(x, x.uid['al'], bob); befriend(x, x.uid['al'], dan); befriend(x, x.uid['al'], fay)

    # Личная переписка al ↔ bob: сообщение уходит на Y.
    _, dm = x.call('al', 'POST', '/api/dm', {'userId': bob}, label='al открывает переписку с bob')
    channel = dm.get('id')
    outbox(x, 'после открытия')
    hold(p)
    _, m1 = x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': f'привет <@{bob}> и `<@{dan}>`'}, label='al пишет')
    outbox(x, 'после сообщения')
    deliver(p)
    outbox(x, 'после доставки')
    remote_view(y, 'bob', 'первое сообщение')

    # Правка, ответ, реакции, удаление.
    hold(p)
    x.call('al', 'PATCH', f"/api/dm/messages/{m1.get('id')}", {'content': 'привет, исправлено'}, label='al правит')
    _, m2 = x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': 'ответ', 'replyToId': m1.get('id')}, label='al отвечает')
    x.ws['al'].send({'type': 'reaction_add', 'messageId': m1.get('id'), 'emoji': '👍'})
    x.ws['al'].send({'type': 'reaction_add', 'messageId': m2.get('id'), 'emoji': '🔥'})
    p.settle(0.5)
    x.ws['al'].send({'type': 'reaction_remove', 'messageId': m2.get('id'), 'emoji': '🔥'})
    p.settle(0.3)
    outbox(x, 'правка, ответ и реакции')
    deliver(p)
    remote_view(y, 'bob', 'правки и ответа')
    hold(p)
    x.call('al', 'DELETE', f"/api/dm/messages/{m2.get('id')}", label='al удаляет ответ')
    # Сообщение, удалённое до отправки, не уходит вовсе.
    x.call('al', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    deliver(p)
    remote_view(y, 'bob', 'удаления')
    hold(p)
    x.ws['al'].send({'type': 'channel_ack', 'channelId': channel, 'messageId': m1.get('id')})
    p.settle(0.4)
    x.call('al', 'DELETE', f'/api/dm/{channel}', label='al закрывает переписку')
    outbox(x, 'позиция чтения и закрытие')
    deliver(p)

    # Группа: создание, название, добавление, исключение, передача, выход.
    hold(p)
    _, group = x.call('al', 'POST', '/api/dm/group', {'users': [{'id': bob}, {'id': dan}]}, label='al создаёт группу')
    gid = group.get('id')
    outbox(x, 'после создания группы')
    deliver(p)
    remote_view(y, 'bob', 'создания группы')
    hold(p)
    x.call('al', 'PATCH', f'/api/dm/{gid}', {'name': 'Команда'}, label='al называет группу')
    x.call('al', 'POST', f'/api/dm/{gid}/members', {'userId': fay}, label='al добавляет fay')
    outbox(x, 'название и новый участник')
    deliver(p)
    remote_view(y, 'fay', 'добавления')
    hold(p)
    x.call('al', 'POST', f'/api/dm/{gid}/messages', {'content': 'всем привет'}, label='al пишет в группу')
    x.call('al', 'DELETE', f'/api/dm/{gid}/members/{dan}', label='al исключает dan')
    x.call('al', 'POST', f'/api/dm/{gid}/transfer', {'newOwnerId': bob}, label='al передаёт группу bob')
    outbox(x, 'исключение и передача')
    deliver(p)
    remote_view(y, 'bob', 'передачи')

    # Дружба. Запрос пользователю другого инстанса идёт через поиск
    # `user@домен` (шаг 5 переноса); здесь — удаление друга.
    hold(p)
    x.call('al', 'DELETE', f'/api/social/friends/{dan}', label='al удаляет dan из друзей')
    outbox(x, 'отзыв и удаление друга')
    deliver(p)

    # Профиль и присутствие.
    hold(p)
    x.call('al', 'PATCH', '/api/users/@me', {'displayName': 'Ал', 'bio': 'тест'}, label='al меняет профиль')
    x.ws['al'].send({'type': 'presence_update', 'status': 'dnd'})
    p.settle(0.4)
    outbox(x, 'профиль и присутствие')
    deliver(p)
    y.call('bob', 'GET', f"/api/users/{y.db('SELECT id FROM users WHERE home_user_id = ?', (x.uid['al'],))[0][0] if y.db('SELECT id FROM users WHERE home_user_id = ?', (x.uid['al'],)) else 'none'}", label='bob видит профиль al')
    outbox(x, 'в конце')
    mutation_log(x, 'в конце')
    x.call('root', 'GET', '/api/federation/peers', label='пиры X в конце')


def set_down(side, down):
    """Делает инстанс недоступным для других (через TLS-фронт)."""
    os.makedirs('/tmp/fedca/down', exist_ok=True)
    path = f'/tmp/fedca/down/{side.host}'
    if down:
        open(path, 'w').close()
    elif os.path.exists(path):
        os.remove(path)


def peer_state(side, label):
    """Состояние пира без точных счётчиков (они зависят от времени)."""
    rows = side.db('SELECT status, consecutive_failures > 0, last_failure_at IS NOT NULL FROM federation_peers')
    queue = side.db('SELECT event_type, attempts > 0 FROM federation_outbox ORDER BY created_at, id')
    side.note(f'пир и очередь {label}', {'peers': [list(r) for r in rows], 'queue': [list(r) for r in queue]})


def wait_for(cond, t):
    deadline = time.time() + t
    while time.time() < deadline:
        if cond():
            return True
        time.sleep(0.3)
    return False


def retries(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root'); x.user('al')
    y.user('bob', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    bob = seed_replica(x, y, 'bob'); befriend(x, x.uid['al'], bob)
    # Копия несуществующего на Y пользователя: Y откажет в событии.
    ghost = snowflake()
    x.exec('INSERT INTO users (id, username, display_name, password_hash, status, is_admin, home_instance, '
           'home_user_id, created_at) VALUES (?, ?, NULL, ?, ?, 0, ?, ?, ?)',
           (ghost, f'ghost{p.tag}@{y.host}', '!federation-replicated', 'offline', y.host, '999999999999999999', int(time.time() * 1000)))
    p.n.tok(ghost); befriend(x, x.uid['al'], ghost)
    _, dm = x.call('al', 'POST', '/api/dm', {'userId': bob}, label='al открывает переписку с bob')
    channel = dm.get('id')

    # Y недоступен: попытки идут с паузами, после десяти неудач пир недоступен.
    set_down(y, True)
    try:
        x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': 'первое'}, label='al пишет при недоступном Y')
        x.call('al', 'POST', f'/api/dm/{channel}/messages', {'content': 'второе'}, label='al пишет ещё')
        wait_for(lambda: x.db('SELECT 1 FROM federation_outbox WHERE attempts > 0'), 5)
        peer_state(x, 'после первых неудач')
        reached = wait_for(lambda: x.db("SELECT 1 FROM federation_peers WHERE status = 'unreachable'"), 40)
        x.note('пир стал недоступным', reached)
        peer_state(x, 'при недоступном пире')
        x.call('root', 'GET', '/api/federation/peers', label='пиры X при недоступном Y')
    finally:
        set_down(y, False)
    # Y вернулся: проверка недоступных пиров возвращает его, очередь уходит.
    back = wait_for(lambda: x.db("SELECT 1 FROM federation_peers WHERE status = 'active'"), 20)
    x.note('пир снова активен', back)
    wait_for(lambda: not x.db('SELECT 1 FROM federation_outbox'), 10)
    p.settle(0.8)
    peer_state(x, 'после возвращения')
    remote_view(y, 'bob', 'после возвращения')

    # Отказ получателя: создание в переписке с неизвестным Y пользователем.
    _, dm2 = x.call('al', 'POST', '/api/dm', {'userId': ghost}, label='al открывает переписку с ghost')
    x.call('al', 'POST', f"/api/dm/{dm2.get('id')}/messages", {'content': 'кто здесь?'}, label='al пишет ghost')
    x.call('al', 'PATCH', '/api/users/@me', {'bio': 'после'}, quiet=True)
    wait_for(lambda: not x.db('SELECT 1 FROM federation_outbox'), 8)
    p.settle(0.5)
    peer_state(x, 'после отказа')
    x.call('root', 'GET', '/api/federation/peers', label='пиры X в конце')


if __name__ == '__main__':
    import sys
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['delivery']
    # Печатание уходит прямым запросом вместе со звонками (шаг 7 переноса).
    sys.exit(1 if sum(run(globals()[n], show=show, ignore_events=('dm_typing_start', 'dm_typing_stop'))
                      for n in names) else 0)

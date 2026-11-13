"""Поиск `user@домен`, заявки в дружбу между инстансами, копии и вход.

`friends`: пользователь X ищет и добавляет в друзья пользователя Y (по имени
и по идентификатору), Y принимает; пользователь Y добавляет пользователя X —
тогда уже Y ищет на X. Копии на X спрашивают свой профиль у дома. Вход в
копию паролем дома (самовосстановление хеша).

`accounts`: прямые подписанные запросы поиска и проверки доказательства,
выдача доказательства, учётные данные для инстанса, удаление своих копий.
"""
import sys, time
from fed import run
from f_inbound import wait_quiet, replicas, ledgers


def requests_view(side, who, label):
    _, incoming = side.call(who, 'GET', '/api/social/requests', quiet=True)
    _, friends = side.call(who, 'GET', '/api/social/friends', quiet=True)
    side.note(f'{who}: заявки и друзья {label}', {'requests': incoming, 'friends': friends})


def friends(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy'); x.user('ed', listen=False)
    y.user('bob'); y.user('dan'); y.user('fay', listen=False)
    y.call('bob', 'PATCH', '/api/users/@me', {'displayName': 'Боб', 'bio': 'с Y'}, quiet=True)
    tag = p.tag
    # Ошибки до пиринга и без него.
    x.call('al', 'POST', '/api/social/requests', {'username': f'bob{tag}@'}, label='имя с пустым доменом')
    x.call('al', 'POST', '/api/social/requests', {'username': f'bob{tag}@bad host'}, label='недопустимый домен')
    x.call('al', 'POST', '/api/social/requests', {'homeUserId': 'x' * 129, 'homeInstance': y.origin}, label='слишком длинный идентификатор')
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    host_y = y.origin.replace('https://', '')
    host_x = x.origin.replace('https://', '')
    # al → bob по имени.
    x.call('al', 'POST', '/api/social/requests', {'username': f'nobody{tag}@{host_y}'}, label='al ищет несуществующего')
    x.call('al', 'POST', '/api/social/requests', {'username': f'BOB{tag}@{host_y}'}, label='al просит bob в друзья')
    x.call('al', 'POST', '/api/social/requests', {'username': f'bob{tag}@{host_y}'}, label='al просит повторно')
    wait_quiet(p)
    requests_view(y, 'bob', 'после заявки al')
    replicas(x, 'после поиска bob')
    # bob принимает на Y.
    _, incoming = y.call('bob', 'GET', '/api/social/requests', quiet=True)
    rid = next((r['id'] for r in incoming if isinstance(r, dict) and r.get('fromId') != y.uid['bob']), 'none') if isinstance(incoming, list) else 'none'
    y.call('bob', 'PATCH', f'/api/social/requests/{rid}', {'status': 'accepted'}, label='bob принимает')
    wait_quiet(p)
    requests_view(x, 'al', 'после ответа bob')
    # cy → dan по идентификатору (с именем рядом).
    x.call('cy', 'POST', '/api/social/requests', {'homeUserId': y.uid['dan'], 'homeInstance': y.origin,
           'username': f'dan{tag}@{host_y}'}, label='cy просит dan по идентификатору')
    x.call('cy', 'POST', '/api/social/requests', {'homeUserId': 'nope', 'homeInstance': host_y}, label='cy: нет такого идентификатора')
    x.call('cy', 'POST', '/api/social/requests', {'homeUserId': x.uid['al'], 'homeInstance': x.origin}, label='cy → al по своему домену')
    wait_quiet(p)
    # dan отвечает отказом.
    _, incoming = y.call('dan', 'GET', '/api/social/requests', quiet=True)
    rid = next((r['id'] for r in incoming if isinstance(r, dict) and r.get('fromId') != y.uid['dan']), 'none') if isinstance(incoming, list) else 'none'
    y.call('dan', 'PATCH', f'/api/social/requests/{rid}', {'status': 'declined'}, label='dan отказывает')
    wait_quiet(p)
    requests_view(x, 'cy', 'после отказа dan')
    # fay (Y) → ed (X): поиск идёт с Y на X.
    y.call('fay', 'POST', '/api/social/requests', {'username': f'ed{tag}@{host_x}'}, label='fay просит ed в друзья')
    y.call('fay', 'POST', '/api/social/requests', {'username': f'zz{tag}@{host_x}'}, label='fay ищет несуществующего на X')
    wait_quiet(p)
    requests_view(x, 'ed', 'после заявки fay')
    # al отменяет и снова просит — уже существующую копию.
    x.call('al', 'DELETE', f"/api/social/friends/{x.db('SELECT id FROM users WHERE username LIKE ?', (f'bob{tag}@%',))[0][0] if x.db('SELECT id FROM users WHERE username LIKE ?', (f'bob{tag}@%',)) else 'none'}", label='al удаляет bob')
    wait_quiet(p)
    requests_view(y, 'bob', 'после удаления')
    # Вход в копию bob на X паролем дома.
    from lib import http
    s, b = http(x.port, 'POST', '/api/auth/login', {'username': f'bob{tag}@{host_y}', 'password': 'wrong-password'})
    p.log.append(('X: вход в копию с неверным паролем', s, p.val(b)))
    s, b = http(x.port, 'POST', '/api/auth/login', {'username': f'bob{tag}@{host_y}', 'password': 'password123'})
    p.log.append(('X: вход в копию паролем дома', s, p.val(b)))
    p.settle(1.5)
    replicas(x, 'в конце')
    ledgers(x, 'в конце')


def accounts(p):
    x, y = p.x, p.y
    p.started = int(time.time() * 1000)
    x.user('root'); y.user('root'); x.user('al'); x.user('ann', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    secret = x.db('SELECT hmac_secret FROM federation_peers')
    secret = secret[0][0] if secret else 'none'
    tag = p.tag
    ask = lambda path, body, label, **kw: x.signed('POST', path, body, secret, y.origin, label, **kw)
    ask('/api/federation/users/lookup', {'username': f' AL{tag} '}, 'поиск по имени')
    ask('/api/federation/users/lookup', {'username': f'nobody{tag}'}, 'поиск несуществующего')
    ask('/api/federation/users/lookup', {'username': 5}, 'поиск: имя не строка')
    ask('/api/federation/users/lookup', {'username': '  '}, 'поиск: пустое имя')
    ask('/api/federation/users/lookup', None, 'поиск: тело null', raw='null')
    ask('/api/federation/users/by-home-id', {'homeUserId': x.uid['al']}, 'по идентификатору')
    ask('/api/federation/users/by-home-id', {'homeUserId': 'nope'}, 'по идентификатору: нет')
    ask('/api/federation/users/by-home-id', {'homeUserId': ''}, 'по идентификатору: пусто')
    ask('/api/federation/users/lookup', {'username': 'x' * 5000}, 'поиск: тело больше 4 КБ')
    x.signed('POST', '/api/federation/users/lookup', {'username': 'al'}, secret, 'https://z.test', 'поиск от неизвестного')
    # Доказательство для привязки.
    x.call('al', 'POST', '/api/auth/attach-proof', {}, label='доказательство без домена')
    x.call('al', 'POST', '/api/auth/attach-proof', {'targetDomain': 'https:///'}, label='доказательство: пустой домен')
    _, proof = x.call('al', 'POST', '/api/auth/attach-proof', {'targetDomain': ' HTTPS://' + y.origin.replace('https://', '').upper() + '/ '}, label='доказательство для Y')
    _, other = x.call('al', 'POST', '/api/auth/attach-proof', {'targetDomain': 'z.test'}, label='доказательство для Z')
    token = proof.get('token', '0' * 64)
    ask('/api/federation/verify-attach-proof', {'token': 'zz'}, 'проверка: не токен')
    ask('/api/federation/verify-attach-proof', {'token': other.get('token', '1' * 64)}, 'проверка: токен для другого домена')
    ask('/api/federation/verify-attach-proof', {'token': token}, 'проверка доказательства')
    ask('/api/federation/verify-attach-proof', {'token': token}, 'проверка повторно')
    x.note('доказательства', [list(r) for r in x.db('SELECT target_domain, used_at IS NOT NULL FROM federation_attach_proofs WHERE created_at >= ? ORDER BY target_domain', (p.started,))])
    # Учётные данные для инстанса.
    x.call('al', 'POST', '/api/users/@me/federation-credential', {}, label='учётные данные без адреса')
    x.call('al', 'POST', '/api/users/@me/federation-credential', {'origin': 'ftp://y'}, label='учётные данные: не http')
    x.call('al', 'POST', '/api/users/@me/federation-credential', {'origin': x.origin}, label='учётные данные для себя')
    x.call('al', 'POST', '/api/users/@me/federation-credential', {'origin': y.origin.upper() + '/'}, label='учётные данные для Y')
    x.call('al', 'POST', '/api/users/@me/federation-credential', {'origin': y.origin, 'markProvisioned': True}, label='учётные данные: выданы')
    # Удаление своих копий на инстансах.
    x.call('al', 'POST', '/api/users/@me/federation-identity/delete', {'origins': [y.origin], 'mode': 'bad'}, label='удаление копий: неверный режим')
    x.call('al', 'POST', '/api/users/@me/federation-identity/delete', {'origins': [], 'mode': 'soft'}, label='удаление копий: пустой список')
    x.call('al', 'POST', '/api/users/@me/federation-identity/delete', {'origins': [y.origin, 'https://z.test'], 'mode': 'soft'}, label='удаление копий')
    x.call('al', 'POST', '/api/users/@me/federation-identity/delete', {'origins': ['https://z.test'], 'mode': 'leave'}, label='удаление копий: уйти')
    x.note('учётные данные', [list(r) for r in x.db('SELECT origin, provisioned_at IS NOT NULL FROM user_federation_credentials WHERE user_id = ?', (x.uid['al'],))])
    x.call('al', 'POST', '/api/users/@me/reattach', {'token': 'zz'}, label='привязка: не токен')
    x.call('al', 'POST', '/api/users/@me/reattach', {'token': 'a' * 64}, label='привязка: аккаунт не отделён')


if __name__ == '__main__':
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['friends', 'accounts']
    # Печатание и звонки между инстансами — шаг 7 переноса.
    sys.exit(1 if sum(run(globals()[n], show=show, ignore_events=('dm_typing_start', 'dm_typing_stop'),
                          ignore_keys=('secret', 'token')) for n in names) else 0)

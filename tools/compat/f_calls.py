"""Звонки и «печатает» между инстансами.

al и cy живут на X, bob и dan — на Y. В паре «ориг» X — оригинал, в паре
«rust» X — сервер на Rust; Y — оригинал (с BS_FED_RUST_Y=1 — тоже Rust).
Звонки идут в обе стороны: вызов, принятие, конец, отказ, вызов, которому
некому ответить, и групповой звонок. Сравниваются события WebSocket обеих
сторон и снимок звонков в `ready`.

Стенд: BS_FED_LIVEKIT=1 tools/compat/fed_up.sh — без ключей медиасервера
вызов на другой инстанс не начинается.
"""
import sys, time
from fed import run
from scn import Listener
from f_outbox import seed_replica, befriend
from f_inbound import wait_quiet


def incoming(side, who, t=5.0):
    """Последний `dm_call_incoming` пользователя."""
    deadline = time.time() + t
    while time.time() < deadline:
        found = [e for e in side.ws[who].events if e.get('type') == 'dm_call_incoming']
        if found:
            return found[-1]
        time.sleep(0.1)
    return {}


def ready_calls(side, who, label):
    """Звонки в `ready` нового соединения."""
    listener = Listener(side.port, side.tok[who])
    listener.stop = True
    calls = sorted(listener.first.get('activeCalls') or [], key=lambda c: c.get('startedAt') or 0)
    side.note(f'{label}: звонки в ready у {who}', calls)


def ws(side, who, event, t=1.2):
    side.ws[who].send(event)
    time.sleep(t)


def one_on_one(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy')
    y.user('bob'); y.user('dan', listen=False)
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    al = seed_replica(y, x, 'al')
    befriend(y, y.uid['bob'], al); befriend(y, y.uid['dan'], al)

    # Переписка bob ↔ al появляется на X с первым сообщением.
    _, dm = y.call('bob', 'POST', '/api/dm', {'userId': al}, quiet=True)
    ych = dm.get('id')
    y.call('bob', 'POST', f'/api/dm/{ych}/messages', {'content': 'привет'}, label='bob пишет')
    wait_quiet(p)
    _, chans = x.call('al', 'GET', '/api/dm', quiet=True)
    xch = next((c['id'] for c in chans if c.get('federatedId') == dm.get('federatedId')), None) if isinstance(chans, list) else None
    x.note('переписка на X найдена', xch is not None)

    # «Печатает» в обе стороны; сообщение снимает его.
    ws(y, 'bob', {'type': 'dm_typing_start', 'dmChannelId': ych})
    ws(x, 'al', {'type': 'dm_typing_start', 'dmChannelId': xch})
    x.call('al', 'POST', f'/api/dm/{xch}/messages', {'content': 'ответ'}, label='al пишет')
    wait_quiet(p)

    # Звонок ведёт X: al звонит, bob принимает на Y, al кладёт трубку.
    ws(x, 'al', {'type': 'dm_call_start', 'dmChannelId': xch}, 1.5)
    fid = incoming(y, 'bob').get('federatedCallId')
    y.note('bob: вызов с ключом', fid is not None)
    ready_calls(y, 'bob', 'вызов от al')
    ws(y, 'bob', {'type': 'dm_call_accept', 'federatedCallId': fid}, 1.5)
    ready_calls(x, 'al', 'звонок идёт')
    ws(x, 'al', {'type': 'dm_call_end', 'dmChannelId': xch}, 1.5)
    ready_calls(y, 'bob', 'после конца')

    # Звонок ведёт Y: bob звонит, al принимает на X и кладёт трубку.
    ws(y, 'bob', {'type': 'dm_call_start', 'dmChannelId': ych}, 1.5)
    ready_calls(x, 'al', 'вызов от bob')
    ws(x, 'al', {'type': 'dm_call_accept', 'dmChannelId': xch}, 1.5)
    ready_calls(x, 'al', 'al в звонке bob')
    ws(x, 'al', {'type': 'dm_call_end', 'dmChannelId': xch}, 1.5)
    ready_calls(y, 'bob', 'после конца звонка bob')

    # Отказы в обе стороны.
    ws(x, 'al', {'type': 'dm_call_start', 'dmChannelId': xch}, 1.5)
    fid = incoming(y, 'bob').get('federatedCallId')
    ws(y, 'bob', {'type': 'dm_call_reject', 'federatedCallId': fid}, 1.5)
    ws(y, 'bob', {'type': 'dm_call_start', 'dmChannelId': ych}, 1.5)
    ws(x, 'al', {'type': 'dm_call_reject', 'dmChannelId': xch}, 1.5)

    # Звонящий передумал, пока звонят.
    ws(x, 'al', {'type': 'dm_call_start', 'dmChannelId': xch}, 1.5)
    ws(x, 'al', {'type': 'dm_call_end', 'dmChannelId': xch}, 1.5)
    ready_calls(y, 'bob', 'после отмены вызова')

    # Вызов, которому некому ответить: dan не на связи.
    _, dm2 = y.call('dan', 'POST', '/api/dm', {'userId': al}, quiet=True)
    y.call('dan', 'POST', f"/api/dm/{dm2.get('id')}/messages", {'content': 'я тут'}, label='dan пишет')
    wait_quiet(p)
    _, chans = x.call('al', 'GET', '/api/dm', quiet=True)
    xch2 = next((c['id'] for c in chans if c.get('federatedId') == dm2.get('federatedId')), None) if isinstance(chans, list) else None
    ws(x, 'al', {'type': 'dm_call_start', 'dmChannelId': xch2}, 2.0)
    ready_calls(x, 'al', 'после недоставленного вызова')


def group(p):
    x, y = p.x, p.y
    x.user('root'); y.user('root')
    x.user('al'); x.user('cy')
    y.user('bob'); y.user('dan')
    x.call('root', 'POST', '/api/federation/peer/initiate', {'remoteOrigin': y.origin}, quiet=True)
    p.settle(1.0)
    al = seed_replica(y, x, 'al'); cy = seed_replica(y, x, 'cy')
    befriend(y, y.uid['bob'], al); befriend(y, y.uid['bob'], cy); befriend(y, y.uid['bob'], y.uid['dan'])

    # Группу заводит bob на Y; на X она появляется с первым сообщением.
    _, g = y.call('bob', 'POST', '/api/dm/group', {'users': [{'id': al}, {'id': cy}, {'id': y.uid['dan']}]}, label='bob создаёт группу')
    gid = g.get('id')
    y.call('bob', 'POST', f'/api/dm/{gid}/messages', {'content': 'всем'}, label='bob пишет в группу')
    wait_quiet(p)
    _, chans = x.call('al', 'GET', '/api/dm', quiet=True)
    xg = next((c['id'] for c in chans if c.get('federatedId') == g.get('federatedId')), None) if isinstance(chans, list) else None
    x.note('группа на X найдена', xg is not None)

    # Звонок ведёт Y: al принимает, cy отказывается, dan входит позже;
    # al выходит, звонок идёт, пока в нём остаются.
    ws(y, 'bob', {'type': 'dm_call_start', 'dmChannelId': gid}, 1.5)
    ready_calls(x, 'cy', 'групповой вызов')
    ws(x, 'al', {'type': 'dm_call_accept', 'dmChannelId': xg}, 1.5)
    ws(x, 'cy', {'type': 'dm_call_reject', 'dmChannelId': xg}, 1.5)
    ready_calls(x, 'cy', 'после отказа cy')
    ws(y, 'dan', {'type': 'dm_call_accept', 'dmChannelId': gid}, 1.5)
    ws(x, 'al', {'type': 'dm_call_end', 'dmChannelId': xg}, 1.5)
    ready_calls(y, 'bob', 'после выхода al')
    ws(y, 'bob', {'type': 'dm_call_end', 'dmChannelId': gid}, 1.0)
    ws(y, 'dan', {'type': 'dm_call_end', 'dmChannelId': gid}, 1.5)
    ready_calls(x, 'al', 'после конца группового')

    # Звонок ведёт X: al звонит в группу, bob принимает на Y, al кладёт трубку.
    ws(x, 'al', {'type': 'dm_call_start', 'dmChannelId': xg}, 1.5)
    fid = incoming(y, 'bob').get('federatedCallId')
    ws(y, 'bob', {'type': 'dm_call_accept', 'federatedCallId': fid}, 1.5)
    ready_calls(x, 'cy', 'cy видит звонок al')
    ws(y, 'dan', {'type': 'dm_call_reject', 'federatedCallId': fid}, 1.5)
    ws(x, 'al', {'type': 'dm_call_end', 'dmChannelId': xg}, 1.5)
    ws(y, 'bob', {'type': 'dm_call_end', 'federatedCallId': fid}, 1.5)
    ready_calls(x, 'cy', 'после конца звонка al')


if __name__ == '__main__':
    show = '-v' in sys.argv
    names = [a for a in sys.argv[1:] if not a.startswith('-')] or ['one_on_one', 'group']
    # Токены медиасервера подписаны со временем выдачи.
    sys.exit(1 if sum(run(globals()[n], show=show, ignore_keys=('livekitToken',)) for n in names) else 0)

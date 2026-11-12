import sys, time; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
from lib import http
from s_files import upload
def adminusers(c):
    a=c.user('al'); b=c.user('bo'); e=c.user('ev')
    c.tok['root']=http(c.port,'POST','/api/auth/login',{'username':'alice','password':'password123'})[1]['token']
    # хранилище
    for path in ['/api/admin/storage/stats','/api/admin/storage/orphans']:
        c.call('al','GET',path); c.call(None,'GET',path)
    att=upload(c,'al','лишний.txt',b'hello')
    did=c.call('al','POST','/api/dm',{'userId':b})['id']
    kept=upload(c,'al','нужный.png',b'not really png')
    c.call('al','POST',f'/api/dm/{did}/messages',{'content':'файл','attachments':[kept['id']]}); c.settle()
    c.call('root','GET','/api/admin/storage/stats')
    o=c.call('root','GET','/api/admin/storage/orphans')
    c.log[-1]=(c.log[-1][0],c.log[-1][1],c.n.val(sorted(x['filename'] for x in o['orphans'])))
    for body in [None,{},{'dryRun':True},{'dryRun':False}]:
        c.call('root','POST','/api/admin/storage/cleanup',body)
    c.call('al','POST','/api/admin/storage/cleanup',{})
    for body in [None,{},{'maxAgeDays':0},{'maxAgeDays':'x'},{'maxAgeDays':'30','dryRun':True},{'maxAgeDays':1}]:
        c.call('root','POST','/api/admin/storage/cleanup-media',body)
    for body in [None,{},{'maxAgeHours':0},{'maxAgeHours':'x'},{'maxAgeHours':-1},{'maxAgeHours':5,'dryRun':True}]:
        c.call('root','POST','/api/admin/storage/cleanup-tus',body)
    c.call('al','POST','/api/admin/storage/cleanup-tus',{})
    c.call('root','GET','/api/admin/storage/stats')
    # пользователи
    tag=c.tag
    for q in [f'?q={tag}',f'?q={tag}&sort=az',f'?q={tag}&sort=za',f'?q={tag}&sort=oldest',f'?q={tag}&pageSize=2',f'?q={tag}&pageSize=2&page=2',
              f'?q={tag}&page=0&pageSize=999',f'?q={tag}&role=admin',f'?q={tag}&role=non-admin',f'?q={tag}&homeInstance=local',
              f'?q={tag}&homeInstance=https%3A%2F%2Fx.example',f'?q={tag}&joinedAfter=2000-01-01',f'?q={tag}&joinedBefore=2000-01-01',
              f'?q={tag}&joinedAfter=junk&joinedBefore=2999-01-01',f'?q={tag}&showDeleted=true']:
        c.call('root','GET','/api/admin/users'+q)
    c.call('al','GET','/api/admin/users'); c.call(None,'GET','/api/admin/users')
    c.call('root','GET','/api/admin/users/instances'); c.call('al','GET','/api/admin/users/instances')
    # роль
    for body in [{},{'isAdmin':'yes'},{'isAdmin':1}]:  # без тела оригинал отвечает 500
        c.call('root','PATCH',f'/api/admin/users/{a}/role',body)
    c.call('root','PATCH','/api/admin/users/nope/role',{'isAdmin':True})
    c.call('al','PATCH',f'/api/admin/users/{a}/role',{'isAdmin':True})
    sp=c.call('bo','POST','/api/spaces',{'name':'Р','avatarColor':'sky'})
    c.call('al','POST','/api/spaces/join',{'inviteCode':sp['inviteCode']}); c.settle()
    c.call('root','PATCH',f'/api/admin/users/{a}/role',{'isAdmin':True}); c.settle()
    c.call('root','PATCH',f'/api/admin/users/{a}/role',{'isAdmin':True}); c.settle()
    c.call('al','GET','/api/admin/users/instances')
    c.call('root','PATCH',f'/api/admin/users/{a}/role',{'isAdmin':False}); c.settle()
    c.call('al','GET','/api/admin/users/instances')
    # сброс пароля
    c.call('al','POST',f'/api/admin/users/{e}/reset-password')
    c.call('root','POST','/api/admin/users/nope/reset-password')
    r=c.call('root','POST',f'/api/admin/users/{e}/reset-password'); c.settle()
    c.log[-1]=(c.log[-1][0],c.log[-1][1],{'length':len(r.get('temporaryPassword',''))})
    c.call('ev','GET','/api/users/@me')
    c.call(None,'POST','/api/auth/login',{'username':'ev'+tag,'password':'password123'})
    s,body=http(c.port,'POST','/api/auth/login',{'username':'ev'+tag,'password':r.get('temporaryPassword','')})
    c.log.append(('вход с временным паролем',s,{'token':'token' in body}))
    # удаление
    c.call('al','DELETE',f'/api/admin/users/{e}')
    c.call('root','DELETE','/api/admin/users/nope')
    root_id=c.call('root','GET','/api/users/@me')['id']
    c.call('root','DELETE',f'/api/admin/users/{root_id}')
    c.call('root','DELETE',f'/api/admin/users/{b}')
    c.call('root','DELETE',f'/api/admin/users/{a}'); c.settle()
    c.call('root','DELETE',f'/api/admin/users/{a}')
    c.call('root','PATCH',f'/api/admin/users/{a}/role',{'isAdmin':True})
    c.call('root','POST',f'/api/admin/users/{a}/reset-password')
    c.call('al','GET','/api/users/@me')
    c.call('root','GET',f'/api/admin/users?q={tag}&showDeleted=true&sort=oldest')
    c.call('bo','GET',f"/api/spaces/{sp['id']}/members")
    c.call('root','POST','/api/admin/test/seed-peer',{'origin':'http://x','hmacSecret':'s'*40})
    # телеметрия и обновления
    c.call('bo','GET','/api/admin/telemetry'); c.call('root','GET','/api/admin/telemetry')
    for body in [None,{},{'enabled':'yes'},{'enabled':False},{'enabled':True},{'enabled':True},{'enabled':False}]:
        c.call('root','PUT','/api/admin/telemetry',body)
    c.call('bo','PUT','/api/admin/telemetry',{'enabled':True})
    p=c.call('root','GET','/api/admin/telemetry/preview'); c.call('bo','GET','/api/admin/telemetry/preview')
    # среда исполнения у серверов разная по определению
    i=[k for k,e in enumerate(c.log) if e[0].endswith('/api/admin/telemetry/preview')][0]
    body=dict(p); body.pop('runtime',None); body['build']=dict(body.get('build',{}),modified=None)
    c.log[i]=(c.log[i][0],c.log[i][1],c.n.val(body))
    c.call('root','GET','/api/admin/instance/update-status'); c.call('root','GET','/api/admin/instance/update-status?refresh=true')
    c.call('bo','GET','/api/admin/instance/update-status')

if __name__=='__main__': sys.exit(1 if run(adminusers, show='-v' in sys.argv) else 0)

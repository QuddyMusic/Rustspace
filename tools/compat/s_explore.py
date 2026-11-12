import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
def explore(c):
    a=c.user('al'); b=c.user('bo'); e=c.user('ev'); m=c.user('mo')
    pub=c.call('al','POST','/api/spaces',{'name':'Открытое Место','avatarColor':'sky'})['id']
    req=c.call('al','POST','/api/spaces',{'name':'По заявке','avatarColor':'sky'})['id']
    prv=c.call('al','POST','/api/spaces',{'name':'Закрытое','avatarColor':'sky'})
    c.call('al','PATCH',f'/api/spaces/{pub}',{'visibility':'public','description':'Про Котов'})
    c.call('al','PATCH',f'/api/spaces/{req}',{'visibility':'request'}); c.settle()
    # управляющий с правом MANAGE_SPACE
    c.call('mo','POST','/api/spaces/join',{'inviteCode':c.call('al','GET',f'/api/spaces/{req}')['inviteCode']}); c.settle()
    role=c.call('al','POST',f'/api/spaces/{req}/roles',{'name':'Упр','permissions':str(1<<5)})
    c.call('al','POST',f"/api/spaces/{req}/members/{m}/roles",{'roleId':role.get('id')}); c.settle()
    for q in ['','?q=','?q=%D0%BA%D0%BE%D1%82%D0%BE%D0%B2','?q=%D0%9C%D0%95%D0%A1%D0%A2%D0%9E','?q=zzz','?limit=1','?limit=1&offset=1','?limit=0&offset=-4','?limit=abc','?limit=999']:
        c.call('bo','GET',f'/api/spaces/explore{q}')
    c.call('al','GET','/api/spaces/explore'); c.call(None,'GET','/api/spaces/explore')
    # открытое вступление
    c.call('bo','POST','/api/spaces/nope/public-join')
    c.call('bo','POST',f"/api/spaces/{prv['id']}/public-join")
    c.call('bo','POST',f'/api/spaces/{req}/public-join')
    c.call('bo','POST',f'/api/spaces/{pub}/public-join'); c.settle()
    c.call('bo','POST',f'/api/spaces/{pub}/public-join')
    c.call('al','POST',f'/api/spaces/{pub}/bans',{'userId':e}); c.settle()
    c.call('ev','POST',f'/api/spaces/{pub}/public-join')
    c.call('bo','GET','/api/spaces/explore')
    # заявки
    c.call('bo','POST','/api/spaces/nope/request-join',{})
    c.call('bo','POST',f'/api/spaces/{pub}/request-join',{})
    c.call('mo','POST',f'/api/spaces/{req}/request-join',{})
    r=c.call('bo','POST',f'/api/spaces/{req}/request-join',{'message':'  пустите  '+'я'*600}); c.settle()
    c.call('bo','POST',f'/api/spaces/{req}/request-join',{'message':'ещё'})
    r2=c.call('ev','POST',f'/api/spaces/{req}/request-join'); c.settle()
    c.call('bo','GET','/api/users/@me/join-requests'); c.call('bo','GET','/api/users/@me/join-requests?status=accepted'); c.call('bo','GET','/api/users/@me/join-requests?status=junk')
    c.call('bo','GET',f'/api/spaces/{req}/join-requests')
    c.call('al','GET','/api/spaces/nope/join-requests')
    c.call('al','GET',f'/api/spaces/{req}/join-requests'); c.call('mo','GET',f'/api/spaces/{req}/join-requests?status=pending')
    c.call('al','GET',f'/api/spaces/{req}/join-requests?status=junk'); c.call('al','GET',f'/api/spaces/{req}/join-requests?status=declined')
    rid=r['id']
    c.call('al','PATCH',f'/api/spaces/{req}/join-requests/{rid}',{})
    c.call('al','PATCH',f'/api/spaces/{req}/join-requests/{rid}',{'action':'maybe'})
    c.call('bo','PATCH',f'/api/spaces/{req}/join-requests/{rid}',{'action':'accept'})
    c.call('al','PATCH',f'/api/spaces/nope/join-requests/{rid}',{'action':'accept'})
    c.call('al','PATCH',f'/api/spaces/{pub}/join-requests/{rid}',{'action':'accept'})
    c.call('al','PATCH',f'/api/spaces/{req}/join-requests/nope',{'action':'accept'})
    c.call('mo','PATCH',f'/api/spaces/{req}/join-requests/{rid}',{'action':'accept'}); c.settle()
    c.call('al','PATCH',f'/api/spaces/{req}/join-requests/{rid}',{'action':'decline'})
    c.call('al','PATCH',f"/api/spaces/{req}/join-requests/{r2['id']}",{'action':'decline'}); c.settle()
    c.call('ev','POST',f'/api/spaces/{req}/request-join',{'message':''}); c.settle()
    c.call('al','GET',f'/api/spaces/{req}/join-requests?status=accepted'); c.call('al','GET',f'/api/spaces/{req}/join-requests?status=declined')
    c.call('ev','GET','/api/users/@me/join-requests'); c.call('bo','GET','/api/users/@me/join-requests?status=accepted')
    c.call('bo','GET',f'/api/spaces/{req}')

if __name__=='__main__': sys.exit(1 if run(explore, show='-v' in sys.argv) else 0)

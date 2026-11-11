import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
def search(c):
    a=c.user('al'); b=c.user('bo'); e=c.user('ev')
    sp=c.call('al','POST','/api/spaces',{'name':'П','avatarColor':'sky'}); sid=sp['id']
    c.call('bo','POST','/api/spaces/join',{'inviteCode':sp['inviteCode']}); c.settle()
    full=c.call('al','GET',f'/api/spaces/{sid}')
    gen=[x for x in full['channels'] if x['type']=='text'][0]['id']
    ids=[]
    for i,(who,text) in enumerate([('al','яблоко раз'),('bo','груша http без адреса'),('al','Яблоко два apple'),
                                   ('bo','APPLE три'),
                                   ('bo','100% сок'),('al','под_черта'),('bo','яблоко четыре')]):
        ids.append(c.call(who,'POST',f'/api/channels/{gen}/messages',{'content':text})['id']); c.settle()
    # has=link, limit=1.7 и повторный q не сравниваются: у оригинала первый не фильтрует, остальные дают 500.
    base=f'/api/channels/{gen}'
    for q in ['','?q=','?q=apple','?q=%D1%8F%D0%B1%D0%BB%D0%BE%D0%BA%D0%BE','?q=%20apple%20','?q=%25','?q=_','?q=%D0%BD%D0%B5%D1%82','?from=bo','?from=BO','?from=b%25',
              '?from=nobody','?from=bo&q=apple','?has=file','?has=image','?has=other','?limit=2','?limit=0',
              '?limit=999','?limit=abc','?limit=2&offset=1','?offset=-5','?offset=100','?before=2000-01-01',
              '?after=2000-01-01','?before=2999-01-01T00:00:00Z','?after=junk','?before=junk&q=apple']:
        c.call('al','GET',f'{base}/search{q}')
    c.call('ev','GET',f'{base}/search?q=apple')
    c.call('al','GET','/api/channels/nope/search?q=apple')
    for q in ['','?messageId=','?messageId=nope',f'?messageId={ids[3]}',f'?messageId={ids[3]}&limit=2',f'?messageId={ids[3]}&limit=3',
              f'?messageId={ids[0]}&limit=4',f'?messageId={ids[6]}&limit=4',f'?messageId={ids[3]}&limit=1',f'?messageId={ids[3]}&limit=0',
              f'?messageId={ids[3]}&limit=500',f'?messageId={ids[3]}&limit=x']:
        c.call('bo','GET',f'{base}/messages/around{q}')
    c.call('ev','GET',f'{base}/messages/around?messageId={ids[3]}')
    c.call('al','GET',f'/api/channels/nope/messages/around?messageId={ids[3]}')
    # личные сообщения
    did=c.call('al','POST','/api/dm',{'userId':b})['id']
    dids=[]
    for who,text in [('al','привет apple'),('bo','ссылка http без адреса'),('al','ещё Apple'),('bo','пока'),('al','финал apple')]:
        dids.append(c.call(who,'POST',f'/api/dm/{did}/messages',{'content':text})['id']); c.settle()
    for q in ['','?q=apple','?from=bo','?from=nobody','?has=file','?limit=1&offset=1','?after=2000-01-01&q=apple','?before=2000-01-01']:
        c.call('bo','GET',f'/api/dm/{did}/search{q}')
    c.call('ev','GET',f'/api/dm/{did}/search?q=apple')
    c.call('al','GET','/api/dm/nope/search?q=apple')
    for q in ['',f'?messageId={dids[2]}',f'?messageId={dids[2]}&limit=2','?messageId=nope',f'?messageId={ids[0]}']:
        c.call('al','GET',f'/api/dm/{did}/messages/around{q}')
    c.call('ev','GET',f'/api/dm/{did}/messages/around?messageId={dids[2]}')
    c.call('al','GET',f'/api/dm/nope/messages/around?messageId={dids[2]}')

if __name__=='__main__': sys.exit(1 if run(search, show='-v' in sys.argv) else 0)

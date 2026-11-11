import sys, asyncio; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from lib import *
P=3105
def reg(name):
    s,b=http(P,'POST','/api/auth/register',{'username':name,'password':'password123'}); assert s in (200,201),(s,b); return b
alice=reg('alice'); bob=reg('bob'); carol=reg('carol')
ta,tb,tc=alice['token'],bob['token'],carol['token']
s,space=http(P,'POST','/api/spaces',{'name':'Видео'},ta); assert s in (200,201),(s,space)
sid=space['id']
s,inv=http(P,'POST',f'/api/spaces/{sid}/invite',{},ta); print('invite',s,inv)
code=(inv or {}).get('inviteCode') or space.get('inviteCode')
print('join',http(P,'POST','/api/spaces/join',{'inviteCode':code},tb)[0])
s,cat=http(P,'POST',f'/api/spaces/{sid}/categories',{'name':'Тайное'},ta); print('cat',s)
s,ch=http(P,'POST',f'/api/spaces/{sid}/channels',{'name':'secret','type':'text','categoryId':cat['id']},ta); print('ch',s,ch if s>=400 else '')
s,r=http(P,'POST',f'/api/spaces/{sid}/roles',{'name':'Мод','color':'#ff0000','permissions':'2054'},ta); print('role',s)
print('assign',http(P,'POST',f"/api/spaces/{sid}/members/{bob['user']['id']}/roles",{'roleId':r['id']},ta)[0])
print('ovr',http(P,'PUT',f"/api/channels/{ch['id']}/overrides",{'targetType':'role','targetId':sid,'allow':'0','deny':'2'},ta)[0])
full=http(P,'GET',f'/api/spaces/{sid}',None,ta)[1]
general=[c for c in full['channels'] if c['type']=='text'][0]['id']
for i in range(3):
    print('msg',http(P,'POST',f'/api/channels/{general}/messages',{'content':f'привет {i}'},ta)[0])
print('msg',http(P,'POST',f'/api/channels/{general}/messages',{'content':'от боба'},tb)[0])
s,dm=http(P,'POST','/api/dm',{'userId':bob['user']['id']},ta); print('dm',s)
print('dmmsg',http(P,'POST',f"/api/dm/{dm['id']}/messages",{'content':'личное'},ta)[0])
print('status',http(P,'PATCH','/api/users/@me',{'customStatus':'занят'},tb)[0])

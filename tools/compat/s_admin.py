import sys; sys.path.insert(0,'/tmp/stand')
from scn import run
from lib import http
def admin(c):
    a=c.user('al')
    # администратор — первый пользователь стенда
    s,b=http(c.port,'POST','/api/auth/login',{'username':'alice','password':'password123'}); c.tok['root']=b['token']; c.n.tok(b['user']['id'])
    sp=c.call('al','POST','/api/spaces',{'name':'У','avatarColor':'sky'}); sid=sp['id']
    full=c.call('al','GET',f'/api/spaces/{sid}'); gen=[x for x in full['channels'] if x['type']=='text'][0]['id']
    c.call('al','GET','/api/users/@me/notification-settings')
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{})
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'level':'loud'})
    c.call('al','PATCH','/api/spaces/nope/notification-settings',{'level':'all'})
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'level':'all','suppressEveryone':True}); c.settle()
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'mute':'indefinite'}); c.settle()
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'suppressRoles':True})
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'level':'nothing'}); c.settle()
    c.call('al','PATCH','/api/channels/nope/notification-settings',{'level':'all'})
    c.call('al','GET','/api/users/@me/notification-settings')
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'mute':None,'level':None,'suppressEveryone':False}); c.settle()
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'level':None}); c.settle()
    c.call('al','GET','/api/users/@me/notification-settings')
    # приглашения
    c.call('al','GET','/api/admin/invites')
    c.call('al','POST','/api/admin/invites',{'name':'x','maxUses':None,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'','maxUses':None,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'x','maxUses':0,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'x','maxUses':None,'expiresAt':5})
    inv=c.call('root','POST','/api/admin/invites',{'name':' Друзьям '+c.tag,'maxUses':1,'expiresAt':None})
    iid=inv['id']
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'name':'Новое '+c.tag,'maxUses':2})
    c.call('root','PATCH','/api/admin/invites/nope',{'name':'x'})
    c.call(None,'GET',f"/api/auth/check-invite?token={inv['token']}",label='check-invite')
    c.call(None,'POST','/api/auth/register',{'username':'inv'+c.tag,'password':'password123','inviteToken':inv['token']},label='register-with-invite')
    c.call('root','GET',f'/api/admin/invites/{iid}/redemptions')
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'maxUses':1})
    c.call('root','POST',f'/api/admin/invites/{iid}/reinstate',{})
    c.call('root','POST',f'/api/admin/invites/{iid}/revoke')
    c.call('root','POST',f'/api/admin/invites/{iid}/revoke')
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'name':'y'})
    c.call('root','POST',f'/api/admin/invites/{iid}/reinstate',{'maxUses':5})
    c.call('root','DELETE',f'/api/admin/invites/{iid}')
    c.call('root','DELETE',f'/api/admin/invites/{iid}')
if __name__=='__main__': sys.exit(1 if run(admin, show='-v' in sys.argv) else 0)

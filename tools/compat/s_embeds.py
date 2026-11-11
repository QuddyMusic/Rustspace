import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
def embeds(c):
    a=c.user('al'); b=c.user('bo')
    sp=c.call('al','POST','/api/spaces',{'name':'В','avatarColor':'sky'}); sid=sp['id']
    c.call('bo','POST','/api/spaces/join',{'inviteCode':sp['inviteCode']}); c.settle()
    gen=[x for x in c.call('al','GET',f'/api/spaces/{sid}')['channels'] if x['type']=='text'][0]['id']
    post=lambda text: (c.call('al','POST',f'/api/channels/{gen}/messages',{'content':text}), c.settle(1.5))[0]
    post('картинка https://pics.invalid/a.PNG?x=1 и ещё раз https://pics.invalid/a.PNG?x=1')
    post('звук https://pics.invalid/a.opus видео https://pics.invalid/b.mov')
    post('страница https://site.invalid/page — без встраивания')
    post('много ' + ' '.join(f'https://pics.invalid/{n}.jpg' for n in range(7)))
    post('sticker:https://x.invalid/api/stickers/assets/apple.webp')
    post('sticker:https://x.invalid/api/stickers/assets/' + 'a'*64 + '.webp')
    post('внутренний http://127.0.0.1:3105/api/instance/info и http://[::1]/x')
    m=post('было https://pics.invalid/old.gif')
    c.call('al','PATCH',f"/api/messages/{m['id']}",{'content':'стало https://pics.invalid/new.webp'}); c.settle(1.5)
    c.call('al','PATCH',f"/api/messages/{m['id']}",{'content':'без ссылок'}); c.settle(1.5)
    c.send('bo',{'type':'message_create','channelId':gen,'content':'по сокету https://pics.invalid/ws.png'}); c.settle(1.5)
    c.call('bo','GET',f'/api/channels/{gen}/messages')
    did=c.call('al','POST','/api/dm',{'userId':b})['id']
    d=c.call('al','POST',f'/api/dm/{did}/messages',{'content':'лс https://pics.invalid/dm.jpeg'}); c.settle(1.5)
    c.call('al','PATCH',f"/api/dm/messages/{d['id']}",{'content':'лс https://pics.invalid/dm2.avif'}); c.settle(1.5)
    c.send('bo',{'type':'dm_message_create','dmChannelId':did,'content':'https://pics.invalid/dm3.mp4'}); c.settle(1.5)
    c.call('bo','GET',f'/api/dm/{did}/messages')
    c.call('al','GET','/api/utils/metadata')
    c.call('al','GET','/api/utils/metadata?url=')
    c.call('al','GET','/api/utils/metadata?url=https%3A%2F%2Fsite.invalid%2Fpage')
    c.call('al','GET','/api/utils/metadata?url=http%3A%2F%2F127.0.0.1%3A3105%2Fapi%2Finstance%2Finfo')
    c.call('al','GET','/api/utils/metadata?url=file%3A%2F%2F%2Fetc%2Fpasswd')
    c.call('al','GET','/api/utils/metadata?url=junk')
    c.call(None,'GET','/api/utils/metadata?url=junk')

if __name__=='__main__': sys.exit(1 if run(embeds, show='-v' in sys.argv) else 0)

"""Isolated local administrator acceptance; no production data or credentials."""
import subprocess,tempfile,os,json,urllib.request,urllib.error,time,sqlite3,socket
with socket.socket() as probe:
 probe.bind(('127.0.0.1',0)); port=probe.getsockname()[1]
origin=f'http://127.0.0.1:{port}'
root=tempfile.mkdtemp(prefix='dh-admin-qa-');bin=os.path.abspath('crates/dh-server/target/debug/dh-server');ui=os.path.abspath('tauri3/dist-web')
code=subprocess.check_output([bin,'init-admin',root],text=True).strip()
p=subprocess.Popen([bin,'serve-rust',root,ui,origin],env={**os.environ,'DH_BIND':f'127.0.0.1:{port}'},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def req(path,data=None,cookie=None):
 headers={'Origin':origin,'Content-Type':'application/json'}
 if cookie:headers['Cookie']=cookie
 r=urllib.request.Request(origin+path,data=None if data is None else json.dumps(data).encode(),headers=headers)
 try:
  with urllib.request.urlopen(r) as response:return response.status,response.read(),response.headers
 except urllib.error.HTTPError as response:return response.code,response.read(),response.headers
try:
 for _ in range(50):
  try:result=req('/api/setup/status');break
  except OSError:time.sleep(.1)
 assert json.loads(result[1])['mode']=='activate'
 assert req('/api/wang/status',{})[0]==401
 payload={'username':'owner','password':'Independent QA passphrase!','code':code}
 assert req('/api/setup/activate',{**payload,'code':'invalid'})[0]==401
 assert req('/api/setup/activate',{**payload,'password':'short'})[0]==400
 assert req('/api/setup/activate',payload)[0]==200
 assert req('/api/setup/activate',payload)[0]==409
 r=req('/api/login',{'username':'OWNER','password':payload['password']});assert r[0]==200
 cookie=r[2]['Set-Cookie'].split(';')[0]
 assert req('/api/session',cookie=cookie)[0]==204
 assert req('/api/admin/credentials',{'username':'newowner','password':'Replacement QA passphrase!','currentPassword':payload['password']},cookie)[0]==200
 assert req('/api/session',cookie=cookie)[0]==401
 r=req('/api/login',{'username':'newowner','password':'Replacement QA passphrase!'});assert r[0]==200
 new_cookie=r[2]['Set-Cookie'].split(';')[0]
 reset=subprocess.check_output([bin,'reset-admin',root],text=True).strip()
 assert req('/api/admin/reset',{'username':'resetowner','password':'Reset QA passphrase!','code':'bad'})[0]==401
 assert req('/api/admin/reset',{'username':'resetowner','password':'Reset QA passphrase!','code':reset})[0]==200
 assert req('/api/session',cookie=new_cookie)[0]==401
 assert req('/api/admin/reset',{'username':'resetowner','password':'Reset QA passphrase!','code':reset})[0]==401
 r=req('/api/login',{'username':'resetowner','password':'Reset QA passphrase!'});assert r[0]==200
 cookie=r[2]['Set-Cookie'].split(';')[0]
 assert req('/api/logout',{},cookie)[0]==204
 assert req('/api/session',cookie=cookie)[0]==401
 # Restart clears management sessions but retains the initialized administrator.
 p.terminate();p.wait(timeout=10)
 p=subprocess.Popen([bin,'serve-rust',root,ui,origin],env={**os.environ,'DH_BIND':f'127.0.0.1:{port}'},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 for _ in range(50):
  try: result=req('/api/setup/status');break
  except OSError:time.sleep(.1)
 assert json.loads(result[1])['mode']=='login'
 assert req('/api/login',{'username':'resetowner','password':'Reset QA passphrase!'})[0]==200
 for _ in range(4):assert req('/api/login',{'username':'nobody','password':'incorrect'})[0]==401
 limited=req('/api/login',{'username':'resetowner','password':'Reset QA passphrase!'})
 assert limited[0]==429 and int(limited[2]['Retry-After'])>0
 assert req('/api/setup/status')[2]['X-Frame-Options']=='DENY'
 db=sqlite3.connect(root+'/admin.db')
 stored=db.execute('SELECT hash FROM admin').fetchone()[0]
 assert stored.startswith('$argon2id$') and 'Reset QA passphrase!' not in stored
 assert not db.execute('SELECT COUNT(*) FROM activation').fetchone()[0]
 db.close()
 print('PASS activation, invalid code/password, replay, login, change, CLI reset, revocation, logout, restart, rate limit, hashed storage')
finally:p.terminate();p.wait(timeout=10)

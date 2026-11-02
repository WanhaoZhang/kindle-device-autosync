"""Bounded preview/export lock retries, with no device or network access."""
from pathlib import Path
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest

HERE=Path(__file__).resolve().parent

class PreviewScriptTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name);self.root=self.base/'root';self.out=self.base/'out';self.kual=self.base/'kual';self.commands=self.base/'commands'
        for path in (self.root/'bin',self.root/'install',self.root/'events/sealed',self.out,self.kual,self.commands):path.mkdir(parents=True,exist_ok=True)
        for name in ('runtime.json','collector.checkpoint'):(self.root/name).write_text('private fixture')
        (self.root/'events/sealed/segment-123-4-5.log').write_text('synthetic reading fixture\n')
        self.env={**os.environ,'TEST_BASE':str(self.base),'PATH':str(self.commands)+':'+os.environ['PATH']}
        self.tool('id','#!/bin/sh\nprintf "0\\n"\n');self.tool('lipc-get-prop','#!/bin/sh\nprintf "1\\n"\n');self.tool('sync','#!/bin/sh\nexit 0\n');self.tool('sleep','#!/bin/sh\nprintf "sleep %s\\n" "$1" >> "$TEST_BASE/calls"\n')
        helper=self.base/'fake.py';helper.write_text('''import json,os,sys
from pathlib import Path
b=Path(os.environ['TEST_BASE']); mode=sys.argv[1]
p=b/(mode+'-count'); n=int(p.read_text()) if p.exists() else 0;p.write_text(str(n+1))
with (b/'calls').open('a') as f:f.write(mode+' '+str(n+1)+'\\n')
values=os.environ.get('TEST_'+mode.upper()+'_SEQ','ok').split(',');state=values[min(n,len(values)-1)]
if mode=='tls':
 v={'private_filesystem_ok':state=='ok','network_contacted':True,'tls_evidence_saved':True,'online_probe':{'state':'completed','https_certificate_validated':state!='ca'}}
 (b/'out/tls-selftest.json').write_text(json.dumps(v,indent=2));raise SystemExit(0 if state=='ok' else 1)
if state!='ok':
 msg={'busy':'worker already running','idle':'idle window not established'}.get(state,'unsafe private state')
 print('worker stopped: '+msg,file=sys.stderr);raise SystemExit(1)
if mode=='export':
 out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True);(out/'sessions.json').write_text('[]')
print('{}')
''')
        script='#!/bin/sh\ncase "$3" in status) mode=status;; operate) mode=operate;; export) mode=export;; *) exit 2;; esac\nif [ "$mode" = export ];then exec '+shlex.quote(sys.executable)+' '+shlex.quote(str(helper))+' "$mode" "$5";else exec '+shlex.quote(sys.executable)+' '+shlex.quote(str(helper))+' "$mode";fi\n'
        self.write(self.root/'bin/ksync-worker',script)
        self.write(self.root/'install/control.sh','#!/bin/sh\nexit 0\n')
        self.write(self.kual/'bootstrap.sh','#!/bin/sh\nexec '+shlex.quote(sys.executable)+' '+shlex.quote(str(helper))+' tls\n')
        for name in ('online-preview.sh','export-audit.sh'):
            body=(HERE/'device/kual'/name).read_text().replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin','PATH='+shlex.quote(self.env['PATH'])).replace('ROOT=/var/local/kindle-native-sync','ROOT='+shlex.quote(str(self.root))).replace('OUT=/mnt/us/.kindle-native-sync-export','OUT='+shlex.quote(str(self.out))).replace('HERE=/mnt/us/extensions/kindle-native-sync','HERE='+shlex.quote(str(self.kual)))
            self.write(self.kual/name,body)

    def write(self,path,text):path.write_text(text);path.chmod(0o700)
    def tool(self,name,text):self.write(self.commands/name,text)
    def count(self,mode):
        p=self.base/(mode+'-count');return int(p.read_text()) if p.exists() else 0
    def run_script(self,name='online-preview.sh',**env):
        return subprocess.run(['sh',str(self.kual/name)],env={**self.env,**env},capture_output=True,text=True,timeout=10)

    def test_busy_status_and_exact_idle_or_busy_prepare_retry(self):
        r=self.run_script(TEST_STATUS_SEQ='busy,ok',TEST_OPERATE_SEQ='idle,busy,ok')
        self.assertEqual(r.returncode,0,r.stderr);self.assertEqual(self.count('tls'),1);self.assertEqual(self.count('operate'),3)

    def test_tls_failed_filesystem_with_proven_busy_repeats_real_probe(self):
        r=self.run_script(TEST_STATUS_SEQ='ok,busy,ok',TEST_TLS_SEQ='race,ok')
        self.assertEqual(r.returncode,0,r.stderr);self.assertEqual(self.count('tls'),2);self.assertEqual(self.count('operate'),1)

    def test_tls_filesystem_failure_without_busy_and_ca_failure_stop(self):
        for state in ('race','ca'):
            with self.subTest(state=state):
                for mode in ('tls','status','operate'):(self.base/(mode+'-count')).unlink(missing_ok=True)
                r=self.run_script(TEST_STATUS_SEQ='ok',TEST_TLS_SEQ=state)
                self.assertNotEqual(r.returncode,0);self.assertEqual(self.count('tls'),1);self.assertEqual(self.count('operate'),0)

    def test_nonbusy_status_and_prepare_failures_are_never_retried(self):
        r=self.run_script(TEST_STATUS_SEQ='bad')
        self.assertNotEqual(r.returncode,0);self.assertEqual(self.count('status'),1);self.assertEqual(self.count('tls'),0)
        (self.base/'status-count').unlink()
        r=self.run_script(TEST_OPERATE_SEQ='bad')
        self.assertNotEqual(r.returncode,0);self.assertEqual(self.count('operate'),1)

    def test_status_busy_is_bounded_and_does_not_contact_tls(self):
        r=self.run_script(TEST_STATUS_SEQ='busy')
        self.assertNotEqual(r.returncode,0);self.assertEqual(self.count('status'),10);self.assertEqual(self.count('tls'),0)

    def test_export_retries_only_busy_and_preserves_sources(self):
        r=self.run_script('export-audit.sh',TEST_EXPORT_SEQ='busy,ok')
        self.assertEqual(r.returncode,0,r.stderr);self.assertEqual(self.count('export'),2)
        self.assertEqual((self.root/'events/sealed/segment-123-4-5.log').read_text(),'synthetic reading fixture\n')
        (self.base/'export-count').unlink()
        r=self.run_script('export-audit.sh',TEST_EXPORT_SEQ='bad')
        self.assertNotEqual(r.returncode,0);self.assertEqual(self.count('export'),1)


if __name__=='__main__':unittest.main()

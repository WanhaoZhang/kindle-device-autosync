"""Configured-preview component repair; temporary paths and fake jobs only."""
from pathlib import Path
import hashlib
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest

HERE=Path(__file__).resolve().parent

class ComponentRepairTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name);self.root=self.base/'root';self.package=self.base/'package';self.out=self.base/'out';self.commands=self.base/'commands';self.commands.mkdir()
        for top in (self.root,self.package):
            for name in ('bin','install'):(top/name).mkdir(parents=True,exist_ok=True)
        (self.root/'events/sealed').mkdir(parents=True)
        self.status=self.base/'old-status.json';self.status.write_text(json.dumps({'attempts':0,'unknown_batches':0,'auto_sync_enabled':False}))
        self.env={**os.environ,'TEST_BASE':str(self.base),'TEST_FREE_KIB':'36827','PATH':str(self.commands)+':'+os.environ['PATH']}
        self.write(self.root/'bin/ksync-worker','#!'+sys.executable+'\nimport os,json\nfrom pathlib import Path\nbase=Path(os.environ["TEST_BASE"])\nwith (base/"calls").open("a") as f:f.write("old-status\\n")\nprint((base/"old-status.json").read_text())\n')
        self.write(self.package/'bin/ksync-worker','#!'+sys.executable+'\nimport os,json\nfrom pathlib import Path\nbase=Path(os.environ["TEST_BASE"])\nwith (base/"calls").open("a") as f:f.write("candidate-selftest\\n")\nprint(json.dumps({"private_filesystem_ok":not bool(os.environ.get("TEST_BAD_SELFTEST")),"network_contacted":False,"actual_device":{"architecture":"arm","serial_prefix":"B0D4","supported_firmware":True,"supported_hardware":True,"root":True}},indent=2))\n')
        for top in (self.root,self.package):
            self.write(top/'bin/ksync-collector','#!/bin/sh\nprintf "collector repair\\n" >> "$TEST_BASE/calls"\n')
            self.write(top/'bin/ksync-event-adapter','#!/bin/sh\n# '+top.name+' adapter\n')
            self.write(top/'install/control.sh','#!/bin/sh\nprintf "control %s\\n" "$1" >> "$TEST_BASE/calls"\n')
        self.write(self.package/'install/common.sh','ROOT='+shlex.quote(str(self.root))+'\nrequire_device() { :; }\nspace_kib() { printf "%s\\n" "$TEST_FREE_KIB"; }\n')
        (self.root/'install/common.sh').write_text('old common\n')
        (self.root/'package-manifest.json').write_text('old manifest\n');(self.package/'manifest.json').write_text('new manifest\n')
        self.private={name:'PRESERVE '+name for name in ('config.json','auth.json','ledger.sqlite','prepared-plan.json','collector.checkpoint')}
        for name,body in self.private.items():(self.root/name).write_text(body)
        self.old={name:(self.root/name).read_bytes() for name in ('bin/ksync-worker','bin/ksync-collector','bin/ksync-event-adapter','install/common.sh','install/control.sh','package-manifest.json')}
        self.tool('lipc-get-prop','#!/bin/sh\nprintf "1\\n"\n')
        self.tool('stop','#!/bin/sh\nprintf "stop %s\\n" "$1" >> "$TEST_BASE/calls"\n')
        self.tool('initctl','#!/bin/sh\nprintf "%s stop/waiting\\n" "$2"\n')
        self.tool('sync','#!/bin/sh\nexit 0\n')
        self.tool('du','#!/bin/sh\ncase "$*" in *ksync-worker*) printf "5120\\tfixture\\n";; *) printf "4\\tfixture\\n";; esac\n')
        self.tool('cp','#!'+sys.executable+'\nimport os,sys\nfrom pathlib import Path\nif os.environ.get("TEST_FAIL_WORKER_COPY") and sys.argv[-1].endswith("bin/ksync-worker.repair"):raise SystemExit(9)\nos.execv("/bin/cp",["/bin/cp",*sys.argv[1:]])\n')
        body=(HERE/'device/kual/repair-components.sh').read_text().replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin','PATH='+shlex.quote(self.env['PATH'])).replace('PACKAGE=/mnt/us/kindle-native-sync-package','PACKAGE='+shlex.quote(str(self.package))).replace('ROOT=/var/local/kindle-native-sync','ROOT='+shlex.quote(str(self.root))).replace('OUT=/mnt/us/.kindle-native-sync-export','OUT='+shlex.quote(str(self.out)))
        self.script=self.base/'repair.sh';self.script.write_text(body)
        self.sums()

    def write(self,path,body):path.write_text(body);path.chmod(0o700)
    def tool(self,name,body):self.write(self.commands/name,body)
    def sums(self):
        paths=sorted(p for p in self.package.rglob('*') if p.is_file() and p.name!='SHA256SUMS')
        (self.package/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+str(p.relative_to(self.package))+'\n' for p in paths))
    def run_repair(self,flag=True,**env):
        return subprocess.run(['sh',str(self.script),*(['--unsubmitted-worker-upgrade'] if flag else [])],env={**self.env,**env},capture_output=True,text=True,timeout=10)
    def assert_private(self):
        for name,body in self.private.items():self.assertEqual((self.root/name).read_text(),body)
    def assert_old(self):
        for name,body in self.old.items():self.assertEqual((self.root/name).read_bytes(),body,name)

    def test_default_still_refuses_different_worker(self):
        self.assertNotEqual(self.run_repair(False).returncode,0);self.assert_old();self.assert_private()
        self.assertFalse((self.root/'disabled').exists())

    def test_explicit_unsubmitted_upgrade_keeps_all_private_state(self):
        result=self.run_repair();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.root/'bin/ksync-worker').read_bytes(),(self.package/'bin/ksync-worker').read_bytes())
        self.assert_private();self.assertFalse((self.root/'.component-repair').exists())
        report=json.loads((self.out/'component-repair-result.json').read_text())
        self.assertFalse(report['worker_unchanged']);self.assertTrue(report['unsubmitted_worker_upgrade'])
        snap=next(self.out.glob('private-repair-*'))
        self.assertEqual((snap/'components/bin_ksync-worker').read_bytes(),self.old['bin/ksync-worker'])
        calls=(self.base/'calls').read_text();self.assertLess(calls.index('stop kindle-native-sync\n'),calls.index('old-status'))
        self.assertLess(calls.index('old-status'),calls.index('candidate-selftest'))

    def test_any_write_unknown_or_enabled_auto_blocks_candidate(self):
        for key,value in [('attempts',1),('unknown_batches',1),('auto_sync_enabled',True)]:
            with self.subTest(key=key):
                state={'attempts':0,'unknown_batches':0,'auto_sync_enabled':False};state[key]=value;self.status.write_text(json.dumps(state,indent=2))
                self.assertNotEqual(self.run_repair().returncode,0);self.assert_old();self.assert_private()
        self.assertNotIn('candidate-selftest',(self.base/'calls').read_text())

    def test_candidate_bad_local_selftest_blocks_replacement(self):
        self.assertNotEqual(self.run_repair(TEST_BAD_SELFTEST='1').returncode,0);self.assert_old();self.assert_private()

    def test_low_space_and_copy_failure_restore_public_components_only(self):
        for env in [{'TEST_FREE_KIB':'25000'},{'TEST_FAIL_WORKER_COPY':'1'}]:
            with self.subTest(env=env):
                # Preserve the unresolved marker from an ordinary rollback.
                (self.root/'.component-repair').unlink(missing_ok=True)
                self.assertNotEqual(self.run_repair(**env).returncode,0);self.assert_old();self.assert_private()
                self.assertTrue((self.root/'disabled').exists());self.assertTrue((self.root/'.component-repair').exists())


if __name__=='__main__':unittest.main()

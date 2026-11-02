"""Reviewed legacy upgrade through temporary public components and fake GETs."""
from pathlib import Path
import hashlib
import json
import os
import shlex
import subprocess
import sys
import unittest
import test_component_repair as fixtures

HERE=Path(__file__).resolve().parent

class VerifiedRuntimeUpgradeTests(unittest.TestCase):
    write=fixtures.ComponentRepairTests.write
    tool=fixtures.ComponentRepairTests.tool
    sums=fixtures.ComponentRepairTests.sums
    assert_private=fixtures.ComponentRepairTests.assert_private
    assert_old=fixtures.ComponentRepairTests.assert_old

    def setUp(self):
        fixtures.ComponentRepairTests.setUp(self)
        self.review=self.base/'review';self.review.mkdir();self.kual=self.base/'kual';self.kual.mkdir()
        self.binding='b'*64;self.plan='a'*64
        self.status.write_text(json.dumps({'attempts':1,'unknown_batches':1,'auto_sync_enabled':False,'binding_sha256':self.binding,'coverage_gap':False,'confirmed_seconds':0},indent=2))
        self.write(self.package/'bin/ksync-worker','#!'+sys.executable+'''\nimport json,os,sys
from pathlib import Path
b=Path(os.environ['TEST_BASE']);command=sys.argv[3]
with (b/'calls').open('a') as f:f.write('candidate '+command+'\\n')
if command=='status':
 s=json.loads((b/'old-status.json').read_text());s['reader_state']=os.environ.get('TEST_RESTART_READER','unknown');print(json.dumps(s,indent=2))
elif command=='self-test':print(json.dumps({'private_filesystem_ok':True,'network_contacted':False,'actual_device':{'root':True,'supported_hardware':True,'supported_firmware':True,'architecture':'arm','serial_prefix':'B0D4'}},indent=2))
elif command=='reconcile':
 p=b/'old-status.json';s=json.loads(p.read_text());s['unknown_batches']=0;s['confirmed_seconds']=4;p.write_text(json.dumps(s,indent=2));print(json.dumps({'state':'confirmed'},indent=2))
else:raise SystemExit(3)
''')
        self.write(self.package/'bin/ksync-collector','#!/bin/sh\nprintf "collector %s\\n" "$*" >> "$TEST_BASE/calls"\n')
        self.write(self.package/'install/control.sh','#!/bin/sh\nprintf "control %s\\n" "$1" >> "$TEST_BASE/calls"\nif [ "$1" = enable ];then rm -f "$TEST_BASE/jobs-stopped";fi\n')
        with (self.package/'install/common.sh').open('a') as f:f.write('ksync_root_mount_state() { printf "read_only\\n"; }\n')
        self.tool('stop','#!/bin/sh\nprintf "stop %s\\n" "$1" >> "$TEST_BASE/calls"\ntouch "$TEST_BASE/jobs-stopped"\n')
        self.tool('initctl','#!/bin/sh\nif [ -e "$TEST_BASE/jobs-stopped" ];then printf "%s stop/waiting\\n" "$2";else case "$2" in kindle-native-sync) pid=4242;; *) pid=4243;; esac;printf "%s start/running, process %s\\n" "$2" "$pid";fi\n')
        self.tool('cp','#!'+sys.executable+'\nimport os,sys\nif os.environ.get("TEST_FAIL_WORKER_COPY") and sys.argv[-1].endswith("bin/ksync-worker.upgrade"):raise SystemExit(9)\nos.execv("/bin/cp",["/bin/cp",*sys.argv[1:]])\n')
        for pid,name in [(4242,'ksync-collector'),(4243,'ksync-event-adapter')]:
            p=self.base/'proc'/str(pid);p.mkdir(parents=True);(p/'exe').symlink_to(self.root/'bin'/name)
        self.write(self.kual/'bootstrap.sh','#!/bin/sh\nprintf "TLS anonymous GET\\n" >> "$TEST_BASE/calls"\n')
        source=(HERE/'device/kual/upgrade-verified-runtime.sh').read_text()
        constants={line.split('=',1)[0]:line.split('=',1)[1] for line in source.splitlines() if line.startswith('LEGACY_')}
        self.legacy=hashlib.sha256((self.root/'bin/ksync-worker').read_bytes()).hexdigest()
        self.manifest={'binaries':{'ksync-worker':{'sha256':self.legacy}},'source_files_sha256':{'device/worker/src/protocol.rs':constants['LEGACY_PROTOCOL'],'device/worker/src/network.rs':constants['LEGACY_NETWORK']}}
        (self.root/'package-manifest.json').write_text(json.dumps(self.manifest,indent=2)+'\n');(self.package/'manifest.json').write_text(json.dumps(self.manifest,indent=2)+'\n')
        self.raw=self.root/'events/sealed/segment-123-4-5.log';self.raw.write_text('native counters fixture\n')
        self.documents=self.base/'documents';self.metric=self.documents/'fixture.sdr/book.azw3f';self.metric.parent.mkdir(parents=True);self.metric.write_text('native sidecar fixture')
        self.policy={'schema_version':1,'kind':'reviewed_native_reading_for_controlled_component_upgrade','legacy_actual_trial_plan_id':self.plan,'config_binding_sha256':self.binding,'legacy_worker_sha256':self.legacy,'legacy_actual_trial_seconds':4,'native_window_counters_continuous':True,'original_trial_must_not_be_reposted':True,'network_writes_performed_by_audit':False}
        (self.review/'controlled-review.json').write_text(json.dumps(self.policy,indent=2)+'\n')
        (self.review/'request.json').write_text(json.dumps({'action':'adopt-and-enable','plan_id':self.plan,'config_binding_sha256':self.binding},indent=2)+'\n')
        (self.review/'raw-SHA256SUMS').write_text(hashlib.sha256(self.raw.read_bytes()).hexdigest()+'  '+self.raw.name+'\n')
        (self.review/'sidecar-SHA256SUMS').write_text(hashlib.sha256(self.metric.read_bytes()).hexdigest()+'  '+str(self.metric)+'\n')
        self.review_sums();self.sums()
        source=source.replace('LEGACY_WORKER='+constants['LEGACY_WORKER'],'LEGACY_WORKER='+self.legacy).replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin','PATH='+shlex.quote(self.env['PATH'])).replace('PACKAGE=/mnt/us/kindle-native-sync-package','PACKAGE='+shlex.quote(str(self.package))).replace('ROOT=/var/local/kindle-native-sync','ROOT='+shlex.quote(str(self.root))).replace('OUT=/mnt/us/.kindle-native-sync-export','OUT='+shlex.quote(str(self.out))).replace('REVIEW=/mnt/us/.kindle-native-sync-upgrade-review','REVIEW='+shlex.quote(str(self.review))).replace('HERE=/mnt/us/extensions/kindle-native-sync','HERE='+shlex.quote(str(self.kual))).replace('/mnt/us/documents',str(self.documents)).replace('/proc/',str(self.base/'proc')+'/')
        self.script=self.base/'verified.sh';self.script.write_text(source)
        self.old={name:(self.root/name).read_bytes() for name in self.old}

    def review_sums(self):
        names=('controlled-review.json','raw-SHA256SUMS','sidecar-SHA256SUMS','request.json')
        (self.review/'SHA256SUMS').write_text(''.join(hashlib.sha256((self.review/n).read_bytes()).hexdigest()+'  '+n+'\n' for n in names))
    def run_upgrade(self,**env):
        return subprocess.run(['sh',str(self.script)],env={**self.env,**env},capture_output=True,text=True,timeout=15)

    def test_verified_upgrade_only_reconciles_and_preserves_private_state(self):
        r=self.run_upgrade();self.assertEqual(r.returncode,0,r.stderr)
        self.assert_private();calls=(self.base/'calls').read_text()
        self.assertEqual(calls.count('candidate reconcile'),1);self.assertNotIn('operate',calls)
        self.assertLess(calls.index('candidate reconcile'),calls.index('stop kindle-native-sync-events'))
        self.assertLess(calls.index('control enable'),calls.index('maintenance acknowledge-gap'))
        self.assertLess(calls.index('maintenance acknowledge-gap'),calls.index('TLS anonymous GET'))
        self.assertEqual((self.root/'controlled-upgrade-reading-review.json').read_bytes(),(self.review/'controlled-review.json').read_bytes())
        report=json.loads((self.out/'verified-runtime-upgrade-result.json').read_text());self.assertFalse(report['account_time_post_performed']);self.assertFalse(report['adoption_performed'])

    def test_other_attempts_or_existing_gap_refuse_before_reconcile(self):
        for field,value in [('attempts',2),('unknown_batches',2),('coverage_gap',True),('auto_sync_enabled',True)]:
            with self.subTest(field=field):
                status={'attempts':1,'unknown_batches':1,'auto_sync_enabled':False,'binding_sha256':self.binding,'coverage_gap':False,'confirmed_seconds':0};status[field]=value;self.status.write_text(json.dumps(status,indent=2))
                self.assertNotEqual(self.run_upgrade().returncode,0);self.assert_old();self.assert_private()
        self.assertNotIn('candidate reconcile',(self.base/'calls').read_text())

    def test_genuine_checkpoint_idle_is_accepted_without_reader_events(self):
        r=self.run_upgrade(TEST_RESTART_READER='idle');self.assertEqual(r.returncode,0,r.stderr)
        self.assert_private();calls=(self.base/'calls').read_text()
        self.assertIn('maintenance acknowledge-gap',calls)
        self.assertNotIn('--emit reader',calls)

    def test_old_binary_or_transport_change_refuses(self):
        self.manifest['source_files_sha256']['device/worker/src/network.rs']='changed'
        (self.package/'manifest.json').write_text(json.dumps(self.manifest,indent=2)+'\n');self.sums()
        self.assertNotEqual(self.run_upgrade().returncode,0);self.assert_old();self.assert_private()
        self.manifest['source_files_sha256']['device/worker/src/network.rs']='1cc550bab418ae47d30b0c987cc9e4281538d17a65bb5a14de3bfaa9404cda73'
        (self.package/'manifest.json').write_text(json.dumps(self.manifest,indent=2)+'\n');self.sums()
        (self.root/'bin/ksync-worker').write_text('other binary')
        self.assertNotEqual(self.run_upgrade().returncode,0);self.assert_private()

    def test_copy_failure_restores_public_but_keeps_reconciled_ledger_and_disabled(self):
        self.assertNotEqual(self.run_upgrade(TEST_FAIL_WORKER_COPY='1').returncode,0)
        self.assert_old();self.assert_private();self.assertTrue((self.root/'disabled').exists());self.assertTrue((self.root/'.verified-runtime-upgrade').exists())
        self.assertEqual(json.loads(self.status.read_text())['confirmed_seconds'],4)
        self.assertNotIn('maintenance acknowledge-gap',(self.base/'calls').read_text())

    def test_changed_sidecar_prevents_acknowledgement(self):
        self.metric.write_text('unreviewed reading')
        self.assertNotEqual(self.run_upgrade().returncode,0);self.assert_old();self.assert_private()
        self.assertFalse((self.root/'disabled').exists())
        self.assertNotIn('maintenance acknowledge-gap',(self.base/'calls').read_text())


if __name__=='__main__':unittest.main()

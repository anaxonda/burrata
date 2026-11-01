#!/usr/bin/env python3
"""Offline provenance mutations; never install a package or launch a container."""
from pathlib import Path
import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('rpm_artifact', Path(__file__).with_name('validate-rpm-artifact.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
COMMIT = 'a' * 40
HARNESS = 'b' * 40


def mcp_record():
    return {'initialized':True,'statusRunning':True,'frontendMounted':True,'toolCount':132}


def evidence(artifact):
    return {**artifact, 'status':'passed', 'sourceRunId':'123', 'harnessCommit':HARNESS,
            'checks':{'rpm':{'postRuntimeVerify':True,'uninstalled':True,'packageAndRuntimePathsAbsent':True,'runtimeUid':10001,'installedVersion':'0.1.25'},
                      'rpmRuntimeCleanup':{'noLiveOwnedProcessesProven':True,'errors':[]},
                      'mcp':mcp_record(),'mcpEntrypoints':{'applicationMcp':mcp_record(),'shippedConsole':[mcp_record()]},
                      'nativeAudio':{'status':'sampled','engine':'native','channels':2,'capturedSeconds':3,
                                     'osOutput':{'postPlayerGain':True,'physicalHardware':False,'verifiedToneSeconds':3.2,'remuteQuietSeconds':.75},
                                     'privateMonitorControl':{'remuteWhilePlaying':True,'remuteClockAdvanceSeconds':1.0}}},
            'rpmArtifactProvenance':{'verifiedReleaseWorkflowRun':True,'repository':'anaxonda/burrata','headSha':COMMIT,'artifactWorkflowHeadSha':COMMIT,'artifactSourceCommit':COMMIT,'runId':'123','image':gate.FEDORA_IMAGE,'harnessCommit':HARNESS}}


class RpmArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name)
        (self.root/'installers').mkdir();self.rpm=self.root/'installers/Burrata-0.1.25-1.x86_64.rpm';self.rpm.write_bytes(b'fixture RPM bytes; no execution')
        self.inventory={'tag':'build-'+COMMIT,'commit':COMMIT,'target':'x86_64-unknown-linux-gnu','label':'linux-x64','artifacts':[{'file':self.rpm.name,'sha256':hashlib.sha256(self.rpm.read_bytes()).hexdigest(),'bytes':self.rpm.stat().st_size}]};self.write()

    def write(self):
        (self.root/'artifact-inventory.json').write_text(json.dumps(self.inventory))

    def artifact(self):
        return gate.downloaded_rpm(self.root,COMMIT,'build-'+COMMIT,'0.1.25')

    def test_exact_artifact_accepts_distinct_explicit_harness_commit(self):
        artifact=self.artifact();gate.validate_evidence(evidence(artifact),artifact)
        self.assertNotEqual(COMMIT,HARNESS)
        item=evidence(artifact);item['rpmArtifactProvenance'].update(headSha='c'*40,artifactWorkflowHeadSha='c'*40)
        gate.validate_evidence(item,artifact)  # release.yml source_ref can differ from workflow head.

    def test_wrong_commit_target_label_tag_or_duplicate_record_rejected(self):
        for key,value in [('commit','c'*40),('target','aarch64-apple-darwin'),('label','macos-arm64'),('tag','v9.9.9')]:
            with self.subTest(key=key):
                original=self.inventory[key];self.inventory[key]=value;self.write()
                with self.assertRaises(ValueError):self.artifact()
                self.inventory[key]=original
        self.inventory['artifacts']*=2;self.write()
        with self.assertRaises(ValueError):self.artifact()

    def test_wrong_hash_size_foreign_extra_or_symlink_artifact_rejected(self):
        self.rpm.write_bytes(b'changed')
        with self.assertRaises(ValueError):self.artifact()
        self.rpm.write_bytes(b'fixture RPM bytes; no execution')
        extra=self.rpm.with_name('foreign.rpm');extra.write_bytes(b'foreign')
        with self.assertRaises(ValueError):self.artifact()
        extra.unlink();self.rpm.unlink();self.rpm.symlink_to(self.root/'artifact-inventory.json')
        with self.assertRaises(ValueError):self.artifact()

    def test_equal_size_tampering_and_boolean_size_are_rejected(self):
        original=self.rpm.read_bytes();self.rpm.write_bytes(b'X'*len(original))
        with self.assertRaises(ValueError):self.artifact()
        self.rpm.write_bytes(original);self.inventory['artifacts'][0]['bytes']=True;self.write()
        with self.assertRaises(ValueError):self.artifact()

    def test_path_traversal_and_option_shaped_identity_rejected(self):
        self.inventory['artifacts'][0]['file']='../foreign.rpm';self.write()
        with self.assertRaises(ValueError):self.artifact()
        with self.assertRaises(ValueError):gate.downloaded_rpm(self.root,'--all','x','0.1.25')

    def test_failed_wrong_source_or_wrong_installer_evidence_rejected(self):
        artifact=self.artifact()
        for key,value in [('status','failed'),('sourceCommit','c'*40),('installerSha256','d'*64),('sourceVersion','9.9.9')]:
            item=evidence(artifact);item[key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):gate.validate_evidence(item,artifact)

    def test_missing_root_or_failed_install_verify_remove_is_not_qualification(self):
        artifact=self.artifact()
        for key,value in [('postRuntimeVerify',False),('uninstalled',False),('packageAndRuntimePathsAbsent',False),('runtimeUid',0),('runtimeUid',True),('installedVersion','9.9.9')]:
            item=evidence(artifact);item['checks']['rpm'][key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):gate.validate_evidence(item,artifact)

    def test_runtime_cleanup_requires_explicit_true_and_typed_empty_errors(self):
        artifact = self.artifact()
        replacements = [None, True, {}, {'errors': []},
                        {'noLiveOwnedProcessesProven': False, 'errors': []},
                        {'noLiveOwnedProcessesProven': 1, 'errors': []},
                        {'noLiveOwnedProcessesProven': True},
                        *({'noLiveOwnedProcessesProven': True, 'errors': value}
                          for value in [None, False, '', {}, (), ['unknown_new_process']])]
        for replacement in replacements:
            with self.subTest(replacement=replacement):
                item = evidence(artifact)
                if replacement is None:
                    del item['checks']['rpmRuntimeCleanup']
                else:
                    item['checks']['rpmRuntimeCleanup'] = replacement
                with self.assertRaises(ValueError): gate.validate_evidence(item, artifact)
        gate.validate_evidence(evidence(artifact), artifact)

    def test_backend_clock_pre_gain_or_invalid_timing_never_passes(self):
        artifact=self.artifact()
        for kind in ['backendOnly','preGain','noRemute','short','nan','infinite','bool']:
            item=evidence(artifact);native=item['checks']['nativeAudio']
            if kind=='backendOnly':item['checks']['mcp']={}
            elif kind=='preGain':native['osOutput']['postPlayerGain']=False
            elif kind=='noRemute':native['privateMonitorControl']['remuteWhilePlaying']=False
            else:native['osOutput']['verifiedToneSeconds']={'short':2.9,'nan':float('nan'),'infinite':float('inf'),'bool':True}[kind]
            with self.subTest(kind=kind),self.assertRaises(ValueError):gate.validate_evidence(item,artifact)

    def test_all_public_mcp_entrypoints_need_real_backend_and_frontend_proof(self):
        artifact=self.artifact()
        for where in ['mcp','applicationMcp','shippedConsole']:
            for key,value in [('initialized',False),('statusRunning',False),('frontendMounted',False),('toolCount',0),('toolCount',True)]:
                item=evidence(artifact)
                record=item['checks']['mcp'] if where=='mcp' else item['checks']['mcpEntrypoints']['applicationMcp'] if where=='applicationMcp' else item['checks']['mcpEntrypoints']['shippedConsole'][0]
                record[key]=value
                with self.subTest(where=where,key=key,value=value),self.assertRaises(ValueError):gate.validate_evidence(item,artifact)
        for replacement in [{}, {'applicationMcp':mcp_record()}, {'applicationMcp':mcp_record(),'shippedConsole':[]}, {'applicationMcp':mcp_record(),'shippedConsole':[{'initialized':True}]}, {'applicationMcp':mcp_record(),'shippedConsole':[mcp_record(),{}]}]:
            item=evidence(artifact);item['checks']['mcpEntrypoints']=replacement
            with self.assertRaises(ValueError):gate.validate_evidence(item,artifact)

    def test_wrong_repository_run_image_or_harness_provenance_rejected(self):
        artifact=self.artifact()
        for key,value in [('verifiedReleaseWorkflowRun',False),('repository','foreign/repo'),('headSha','c'*40),('artifactSourceCommit','c'*40),('runId','999'),('image','fedora:latest'),('harnessCommit','c'*40)]:
            item=evidence(artifact);item['rpmArtifactProvenance'][key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):gate.validate_evidence(item,artifact)


if __name__ == '__main__':
    unittest.main()

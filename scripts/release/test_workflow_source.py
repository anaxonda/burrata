#!/usr/bin/env python3
"""Execute release.yml's actual validation shell against offline Git fixtures."""
from pathlib import Path
import json
import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/release.yml'


def validation_script():
    text = WORKFLOW.read_text()
    step = text.split('name: Validate source commit and package versions\n', 1)[1]
    block = re.search(r'^        run: \|\n((?:          [^\n]*\n|[ \t]*\n)+)', step, re.MULTILINE)
    if block is None:
        raise AssertionError('Could not locate actual validation run block')
    return textwrap.dedent(block[1])


class WorkflowSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.seed_temp = tempfile.TemporaryDirectory(prefix='burrata-workflow-seed-')
        cls.seed = Path(cls.seed_temp.name)
        cls.env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)
        cls.git(cls.seed, 'init', '--initial-branch=main')
        cls.git(cls.seed, 'config', 'user.name', 'Workflow source fixture')
        cls.git(cls.seed, 'config', 'user.email', 'workflow-fixture@example.invalid')
        (cls.seed / 'src-tauri').mkdir()
        (cls.seed / 'package.json').write_text(json.dumps({'version': '1.2.3'}))
        (cls.seed / 'src-tauri/tauri.conf.json').write_text(json.dumps({'version': '1.2.3'}))
        (cls.seed / 'src-tauri/Cargo.toml').write_text('[package]\nversion = "1.2.3"\n')
        cls.git(cls.seed, 'add', '.')
        cls.git(cls.seed, 'commit', '-m', 'Offline main fixture')
        cls.main_commit = cls.git(cls.seed, 'rev-parse', 'HEAD').strip()
        cls.git(cls.seed, 'update-ref', 'refs/remotes/origin/main', cls.main_commit)
        cls.git(cls.seed, 'tag', 'v1.2.3')
        cls.git(cls.seed, 'tag', 'v1.2.4')
        cls.git(cls.seed, 'checkout', '-b', 'qualification/source')
        (cls.seed / 'qualification.txt').write_text('Unpublished source fixture\n')
        cls.git(cls.seed, 'add', '.')
        cls.git(cls.seed, 'commit', '-m', 'Offline qualification fixture')
        cls.source_commit = cls.git(cls.seed, 'rev-parse', 'HEAD').strip()
        cls.git(cls.seed, 'update-ref', 'refs/remotes/origin/qualification/source', cls.source_commit)
        cls.git(cls.seed, 'checkout', 'main')
        cls.script = validation_script()
        subprocess.run(['bash', '-n'], input=cls.script, text=True, check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.seed_temp.cleanup()

    @classmethod
    def git(cls, cwd, *args):
        result = subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', *args],
                                cwd=cwd, env=cls.env, text=True, capture_output=True, check=True)
        return result.stdout

    def validate(self, *, head='main', source='', tag='', event='workflow_dispatch', extensions=False, linux_only=False):
        with tempfile.TemporaryDirectory(prefix='burrata-workflow-case-') as directory:
            repo = Path(directory) / 'repo'
            shutil.copytree(self.seed, repo)
            self.git(repo, 'checkout', '--detach', head)
            output = Path(directory) / 'github-output'
            env = dict(self.env, REQUESTED_TAG=tag, REQUESTED_SOURCE_REF=source, WORKFLOW_EVENT=event,
                       EXTENSIONS_ONLY=str(extensions).lower(), LINUX_ONLY=str(linux_only).lower(), GITHUB_OUTPUT=str(output))
            result = subprocess.run(['bash', '-c', self.script], cwd=repo, env=env,
                                    text=True, capture_output=True, timeout=10)
            values = dict(line.split('=', 1) for line in output.read_text().splitlines()) if output.exists() else {}
            return result, values

    def assert_rejected(self, **kwargs):
        result, output = self.validate(**kwargs)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, {})

    def test_default_dispatch_requires_current_origin_main_and_never_publishes(self):
        result, output = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['commit'], self.main_commit)
        self.assertEqual(output['tag'], 'build-' + self.main_commit)
        self.assertEqual(output['publish_draft'], 'false')

    def test_default_dispatch_rejects_non_main_checkout(self):
        self.assert_rejected(head=self.source_commit)

    def test_explicit_branch_qualifies_actual_checked_commit_without_publishing(self):
        result, output = self.validate(head=self.source_commit, source='qualification/source')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['commit'], self.source_commit)
        self.assertEqual(output['tag'], 'build-' + self.source_commit)
        self.assertEqual(output['publish_draft'], 'false')

    def test_explicit_commit_qualifies_detached_checkout_without_publishing(self):
        result, output = self.validate(head=self.source_commit, source=self.source_commit)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['commit'], self.source_commit)
        self.assertEqual(output['publish_draft'], 'false')

    def test_explicit_source_rejects_wrong_checked_commit(self):
        self.assert_rejected(source=self.source_commit)
        self.assert_rejected(source='qualification/source')

    def test_source_ref_resolving_to_a_tagged_commit_still_cannot_publish(self):
        result, output = self.validate(source='v1.2.3')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['commit'], self.main_commit)
        self.assertEqual(output['publish_draft'], 'false')
        self.assertEqual(output['tag'], 'build-' + self.main_commit)

    def test_unknown_source_and_option_shaped_source_are_rejected(self):
        self.assert_rejected(source='missing-source')
        self.assert_rejected(source='--all')

    def test_tag_and_source_ref_cannot_be_combined(self):
        result, output = self.validate(tag='v1.2.3', source=self.main_commit)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('source_ref cannot be combined', result.stderr)
        self.assertEqual(output, {})

    def test_manual_existing_release_tag_retains_strict_version_and_publish_guard(self):
        result, output = self.validate(tag='v1.2.3')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['tag'], 'v1.2.3')
        self.assertEqual(output['publish_draft'], 'true')
        self.assertEqual(output['commit'], self.main_commit)

    def test_pushed_release_tag_retains_strict_publish_guard(self):
        result, output = self.validate(tag='v1.2.3', event='push')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['publish_draft'], 'true')

    def test_tag_must_point_to_checked_commit_and_match_package_version(self):
        self.assert_rejected(head=self.source_commit, tag='v1.2.3')
        self.assert_rejected(tag='v1.2.4')
        self.assert_rejected(tag='not-a-version')

    def test_non_dispatch_source_and_untagged_push_are_rejected(self):
        self.assert_rejected(source=self.source_commit, head=self.source_commit, event='push')
        self.assert_rejected(event='push')

    def test_extensions_only_source_qualification_still_cannot_publish(self):
        result, output = self.validate(head=self.source_commit, source='qualification/source', extensions=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['extensions_only'], 'true')
        self.assertEqual(output['publish_draft'], 'false')
        self.assert_rejected(tag='v1.2.3', extensions=True)

    def test_default_and_tag_builds_retain_the_existing_three_platform_matrix(self):
        for kwargs in [{}, {'tag': 'v1.2.3'}, {'tag': 'v1.2.3', 'event': 'push'}]:
            result, output = self.validate(**kwargs)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output['linux_only'], 'false')
            self.assertEqual([row['label'] for row in json.loads(output['build_matrix'])['include']],
                             ['macos-arm64', 'windows-x64', 'linux-x64'])

    def test_linux_only_dispatch_selects_exact_existing_linux_job_without_publishing(self):
        result, output = self.validate(head=self.source_commit, source=self.source_commit, linux_only=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['publish_draft'], 'false')
        self.assertEqual(output['extensions_only'], 'false')
        self.assertEqual(output['linux_only'], 'true')
        self.assertEqual(json.loads(output['build_matrix']), {'include': [{
            'target': 'x86_64-unknown-linux-gnu', 'os': 'ubuntu-22.04',
            'bundles': 'deb,rpm,appimage', 'label': 'linux-x64'}]})

    def test_linux_only_cannot_trigger_tag_extension_or_push_qualification(self):
        self.assert_rejected(tag='v1.2.3', linux_only=True)
        self.assert_rejected(extensions=True, linux_only=True)
        self.assert_rejected(event='push', linux_only=True)

    def test_checkout_selects_source_and_downstream_jobs_use_validated_commit(self):
        workflow = WORKFLOW.read_text()
        self.assertIn("(inputs.tag || inputs.source_ref || 'main')", workflow)
        self.assertEqual(workflow.count('ref: ${{ needs.prepare.outputs.commit }}'), 3)
        self.assertIn("if: needs.prepare.outputs.publish_draft == 'true'", workflow)
        self.assertIn('matrix: ${{ fromJSON(needs.prepare.outputs.build_matrix) }}', workflow)
        self.assertIn("needs.prepare.outputs.linux_only != 'true' &&", workflow)



class RpmWorkflowTests(unittest.TestCase):
    def test_gate_is_required_for_draft_and_uses_same_build_artifact(self):
        source=WORKFLOW.read_text();rpm=(ROOT/'.github/workflows/qualify-rpm.yml').read_text()
        self.assertIn('needs: [prepare, build, extension, rpm-installed]',source)
        gate=source.split('  rpm-installed:\n',1)[1].split('  extension:',1)[0]
        self.assertIn('needs: [prepare, build]',gate)
        self.assertIn('uses: ./.github/workflows/qualify-rpm.yml',gate)
        self.assertIn("if: needs.prepare.outputs.extensions_only != 'true'",gate)
        self.assertIn('source_commit: ${{ needs.prepare.outputs.commit }}',gate)
        self.assertIn('run-id: ${{ inputs.artifact_run_id }}',rpm)
        self.assertIn('name: burrata-linux-x64-${{ inputs.source_commit }}',rpm)
        self.assertIn('runuser -u qualifier',rpm)
        self.assertIn('test -z "${WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS:-}"',rpm)
        self.assertNotIn('--privileged',rpm)
        self.assertNotIn('--no-sandbox',rpm)

    def test_registered_diagnostic_bootstrap_preserves_deb_and_forwards_rpm(self):
        text=(ROOT/'.github/workflows/diagnose-installed-linux.yml').read_text()
        self.assertIn('      rpm_only:\n',text)
        option=text.split('      rpm_only:\n',1)[1].split('      rpm_version:',1)[0]
        self.assertIn('type: boolean',option);self.assertIn('default: false',option)
        legacy=text.split('  replay-linux-smoke:\n',1)[1].split('\n  rpm-installed:',1)[0]
        self.assertIn('    if: ${{ !inputs.rpm_only }}\n',legacy)
        legacy=legacy.replace('    if: ${{ !inputs.rpm_only }}\n','',1).rstrip()
        self.assertEqual(hashlib.sha256(legacy.encode()).hexdigest(),'6bf4df863c81a34af6f055471562b6d4f3c347ee197b7b16ea05b302fbc4e8ca')
        gate=text.split('  rpm-installed:\n',1)[1]
        self.assertIn('    if: inputs.rpm_only',gate)
        self.assertIn('uses: ./.github/workflows/qualify-rpm.yml',gate)
        self.assertNotIn('artifact_name',gate)
        for mapping in ['artifact_run_id: ${{ inputs.source_run_id }}','source_commit: ${{ inputs.expected_commit }}','version: ${{ inputs.rpm_version }}','tag: ${{ inputs.rpm_tag }}']:
            self.assertIn(mapping,gate)

    def upstream(self, **changes):
        text=(ROOT/'.github/workflows/qualify-rpm.yml').read_text()
        block=text.split('          script: |\n',1)[1].split('      - uses: actions/download-artifact',1)[0]
        code=textwrap.dedent(block)
        data={'id':'122','sha':'a'*40,'repo':'anaxonda/burrata','workflow':7,'status':'completed','conclusion':'success','artifactSha':'a'*40,'runSha':'a'*40,'expired':False,'duplicate':False,'contextRun':123}
        data.update(changes)
        wrapper="""const input=JSON.parse(process.argv[1]);const code=process.argv[2];let saved;
const artifact={name:'burrata-linux-x64-'+input.sha,expired:input.expired,id:1,workflow_run:{id:Number(input.id),head_sha:input.artifactSha},digest:'sha256:fixture'};
const github={rest:{actions:{getWorkflow:async()=>({data:{id:7}}),getWorkflowRun:async()=>({data:{id:Number(input.id),workflow_id:input.workflow,head_sha:input.runSha,status:input.status,conclusion:input.conclusion,repository:{full_name:input.repo}}}),listWorkflowRunArtifacts:()=>{}}},paginate:async()=>input.duplicate?[artifact,artifact]:[artifact]};
const context={repo:{owner:'anaxonda',repo:'burrata'},runId:input.contextRun,sha:'b'.repeat(40)};
const fakeProcess={env:{ARTIFACT_RUN_ID:input.id,ARTIFACT_COMMIT:input.sha,ARTIFACT_VERSION:'0.1.25',ARTIFACT_TAG:'build-'+input.sha}};
const fakeRequire=()=>({mkdirSync:()=>{},writeFileSync:(p,v)=>saved=JSON.parse(v)});
const Async=Object.getPrototypeOf(async function(){}).constructor;
try{await new Async('github','context','process','require',code)(github,context,fakeProcess,fakeRequire);process.stdout.write(JSON.stringify(saved));}catch(e){process.stderr.write(e.message);process.exitCode=1;}
"""
        return subprocess.run(['node','--input-type=module','-e',wrapper,json.dumps(data),code],capture_output=True,text=True,timeout=5)

    def test_actual_upstream_script_accepts_cross_run_distinct_harness_and_same_run(self):
        result=self.upstream();self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout);self.assertEqual(data['headSha'],'a'*40);self.assertEqual(data['harnessCommit'],'b'*40)
        result=self.upstream(runSha='c'*40,artifactSha='c'*40)
        self.assertEqual(result.returncode,0,result.stderr)  # explicit source_ref differing from run head
        result=self.upstream(id='123',status='in_progress',conclusion=None)
        self.assertEqual(result.returncode,0,result.stderr)

    def test_actual_upstream_script_rejects_foreign_failed_expired_ambiguous_sources(self):
        for changes in [{'repo':'foreign/repo'},{'workflow':8},{'runSha':'c'*40},{'status':'in_progress'},{'conclusion':'failure'},{'artifactSha':'c'*40},{'expired':True},{'duplicate':True},{'sha':'--all'}]:
            with self.subTest(changes=changes):
                result=self.upstream(**changes);self.assertNotEqual(result.returncode,0);self.assertEqual(result.stdout,'')

if __name__ == '__main__':
    unittest.main()

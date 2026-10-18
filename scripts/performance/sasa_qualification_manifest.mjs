import {lstat,readFile,realpath} from 'node:fs/promises';
import {basename,dirname,join,resolve,relative,isAbsolute} from 'node:path';
import {createHash} from 'node:crypto';
const validId=value=>typeof value==='string'&&/^[A-Za-z0-9._:-]{1,100}$/.test(value);
export function validateSasaFixture(fixture){
 if(!fixture||!validId(fixture.trackId)||!Number.isFinite(fixture.durationSeconds)||fixture.durationSeconds<10
   ||!Array.isArray(fixture.sasaCases)||fixture.sasaCases.length!==2
   ||!fixture.sasaCases.every(testCase=>validId(testCase.trackId)&&testCase.expectedDurationSeconds===281)
   ||new Set(fixture.sasaCases.map(testCase=>testCase.trackId)).size!==2
   ||!fixture.sasaCases.some(testCase=>testCase.metadataShape==='separate_fields')
   ||!fixture.sasaCases.some(testCase=>testCase.metadataShape==='merged_title')
   ||!validId(fixture.remoteCase?.trackId)||!Number.isFinite(fixture.remoteCase.expectedDurationSeconds)
   ||fixture.remoteCase.expectedDurationSeconds<=5)throw Error('invalid_sasa_fixture');
 return fixture;
}
export async function validateSasaManifest(manifestFile){
 const manifest=JSON.parse(await readFile(manifestFile,'utf8'));
 if(manifest.schemaVersion!==1||typeof manifest.automationFile!=='string'||typeof manifest.binary!=='string'
   ||typeof manifest.fixtureFile!=='string'||!/^[a-f0-9]{64}$/.test(manifest.binarySha256))throw Error('invalid_sasa_manifest');
 const automationFile=await realpath(resolve(manifest.automationFile));
 const root=resolve(dirname(automationFile),'../..');
 if(dirname(root)!=='/tmp'||!/^burrata-launch\.[A-Za-z0-9_-]+$/.test(basename(root))
   ||automationFile!==join(root,'data/org.burrata.discovery/automation.json'))throw Error('isolated_profile_required');
 const marker=join(root,'.burrata-isolated-profile'),info=await lstat(marker);
 if(info.isSymbolicLink()||!info.isFile()||await readFile(marker,'utf8')!=='burrata isolated memory run\n')throw Error('isolated_profile_required');
 const fixtureFile=await realpath(resolve(manifest.fixtureFile));
 const fixtureRelative=relative(join(root,'fixture'),fixtureFile);
 if(!fixtureRelative||fixtureRelative==='..'||fixtureRelative.startsWith('../')||isAbsolute(fixtureRelative))throw Error('isolated_fixture_required');
 const binaryFile=await realpath(resolve(manifest.binary));
 if(createHash('sha256').update(await readFile(binaryFile)).digest('hex')!==manifest.binarySha256)throw Error('binary_hash_mismatch');
 return {automationFile,binaryFile,fixture:validateSasaFixture(JSON.parse(await readFile(fixtureFile,'utf8')))};
}

#!/usr/bin/env python3
"""Bounded private AT-SPI registry setup for the isolated QueueSelected UI run."""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

PROFILE_MARKER='burrata isolated memory run\n'

def proc_start_time(pid):
    raw=Path(f'/proc/{pid}/stat').read_text()
    return raw[raw.rfind(')')+2:].split()[19]

def appimage_sha256(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda:stream.read(1024*1024),b''):digest.update(block)
    return digest.hexdigest()

def prepare_appimage_launch(image, environment):
    image=Path(image).resolve(strict=True)
    digest=appimage_sha256(image)
    launch_monotonic_ns=time.monotonic_ns()
    launch_unix_ms=time.time_ns()//1_000_000
    environment['APPIMAGE']=str(image)
    environment['BURRATA_QUAL_APP_LAUNCH_MONOTONIC_NS']=str(launch_monotonic_ns)
    return image,digest,launch_monotonic_ns,launch_unix_ms

def write_launch_clock_receipt(profile, receipt_path, image, image_sha256, launcher_pid, launcher_start_ticks,
                               appimage_pid, appimage_start_ticks, launch_monotonic_ns, launch_unix_ms):
    profile=Path(profile).resolve(strict=True);receipt_path=Path(receipt_path)
    if receipt_path!=profile/'app-launch-clock.json' or receipt_path.is_symlink() or receipt_path.exists():
        raise RuntimeError('PRIVATE_LAUNCH_CLOCK_PATH_INVALID')
    image=Path(image).resolve(strict=True)
    payload={'schemaVersion':1,'profile':str(profile),'appImagePath':str(image),
        'appImageSha256':image_sha256,'launcherPid':launcher_pid,
        'launcherStartTimeTicks':str(launcher_start_ticks),'appImagePid':appimage_pid,
        'appImageStartTimeTicks':str(appimage_start_ticks),'appLaunchMonotonicNs':launch_monotonic_ns,
        'appLaunchUnixMs':launch_unix_ms}
    fd=os.open(receipt_path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|getattr(os,'O_NOFOLLOW',0),0o600)
    try:
        with os.fdopen(fd,'w',encoding='utf-8') as stream:
            stream.write(json.dumps(payload,sort_keys=True)+'\n');stream.flush();os.fsync(stream.fileno())
    except BaseException:
        try:receipt_path.unlink()
        except OSError:pass
        raise
    return payload

def dbus_name_peer(address,name):
    owner=subprocess.run(['gdbus','call','--address',address,'--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus',
        '--method','org.freedesktop.DBus.GetNameOwner',name],capture_output=True,text=True,timeout=3)
    if owner.returncode:
        return None
    try:
        unique=ast.literal_eval(owner.stdout.strip())[0]
    except Exception:
        return None
    pid_result=subprocess.run(['gdbus','call','--address',address,'--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus',
        '--method','org.freedesktop.DBus.GetConnectionUnixProcessID',unique],capture_output=True,text=True,timeout=3)
    uid_result=subprocess.run(['gdbus','call','--address',address,'--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus',
        '--method','org.freedesktop.DBus.GetConnectionUnixUser',unique],capture_output=True,text=True,timeout=3)
    pid_match=re.search(r'uint32\s+(\d+)',pid_result.stdout)
    uid_match=re.search(r'uint32\s+(\d+)',uid_result.stdout)
    if pid_result.returncode or uid_result.returncode or not pid_match or not uid_match:
        return None
    return {'pid':int(pid_match.group(1)),'uid':int(uid_match.group(1))}

def accessibility_address_kind(address):
    if not isinstance(address,str) or len(address)>1024:
        return None
    pieces=address.split(',')
    if len(pieces)>2 or any(not re.fullmatch(r'guid=[A-Fa-f0-9]+',option) for option in pieces[1:]):
        return None
    endpoint=pieces[0]
    for prefix,kind in (('unix:path=/tmp/','unix-path'),('unix:abstract=/tmp/','unix-abstract')):
        if endpoint.startswith(prefix):
            name=endpoint.removeprefix(prefix)
            components=name.split('/')
            if name and all(component and component not in {'.','..'}
                            and re.fullmatch(r'[A-Za-z0-9_.-]+',component) for component in components):
                return kind
    return None

def validate_private_runtime_socket(address):
    runtime_value=os.environ.get('XDG_RUNTIME_DIR','')
    runtime=Path(runtime_value)
    if not runtime.is_absolute() or runtime.is_symlink() or runtime.resolve(strict=True)!=runtime:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_DIR_INVALID')
    runtime_stat=runtime.stat()
    if runtime_stat.st_uid!=os.getuid() or runtime_stat.st_mode&0o077:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_DIR_PERMISSIONS_INVALID')
    atspi_dir=runtime/'at-spi'
    if atspi_dir.is_symlink() or not atspi_dir.is_dir() or atspi_dir.resolve(strict=True)!=atspi_dir:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_SOCKET_DIR_INVALID')
    dir_stat=atspi_dir.stat()
    if dir_stat.st_uid!=os.getuid() or dir_stat.st_mode&0o077:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_SOCKET_DIR_PERMISSIONS_INVALID')
    socket_path=Path(address.removeprefix('unix:path=').split(',',1)[0])
    if socket_path.is_symlink() or socket_path.parent!=atspi_dir:
        raise RuntimeError('PRIVATE_ATSPI_SOCKET_NOT_IN_PRIVATE_RUNTIME')
    if not socket_path.is_socket() or socket_path.stat().st_uid!=os.getuid():
        raise RuntimeError('PRIVATE_ATSPI_SOCKET_INVALID')

def owned_bus_peer(session, profile):
    peer=dbus_name_peer(session,'org.a11y.Bus')
    if not peer or peer['uid']!=os.getuid():
        return None
    pid=peer['pid']
    try:
        exe=Path(f'/proc/{pid}/exe').resolve(strict=True)
        if exe!=Path('/usr/lib/at-spi-bus-launcher').resolve(strict=True):
            return None
        env=Path(f'/proc/{pid}/environ').read_bytes().split(b'\0')
        values={item.split(b'=',1)[0]:item.split(b'=',1)[1] for item in env if b'=' in item}
        if values.get(b'XDG_RUNTIME_DIR')!=str(profile/'runtime').encode():
            return None
        if session.encode() not in (values.get(b'DBUS_SESSION_BUS_ADDRESS'),values.get(b'DBUS_STARTER_ADDRESS')):
            return None
        start=proc_start_time(pid)
    except OSError:
        return None
    return {'pid':pid,'uid':peer['uid'],'startTimeTicks':start}

def call_address(session):
    if not session.startswith('unix:path=/tmp/dbus-'):
        raise RuntimeError('PRIVATE_SESSION_BUS_INVALID')
    result=subprocess.run(['gdbus','call','--address',session,'--dest','org.a11y.Bus','--object-path','/org/a11y/bus','--method','org.a11y.Bus.GetAddress'],
        capture_output=True,text=True,timeout=8)
    if result.returncode:
        raise RuntimeError('PRIVATE_ATSPI_BUS_SERVICE_UNAVAILABLE')
    try:
        address=ast.literal_eval(result.stdout.strip())[0]
    except Exception:
        raise RuntimeError('PRIVATE_ATSPI_ADDRESS_INVALID') from None
    kind=accessibility_address_kind(address)
    if kind is None:
        raise RuntimeError('PRIVATE_ATSPI_ADDRESS_NOT_PRIVATE')
    if kind=='unix-path':
        validate_private_runtime_socket(address)
    elif kind=='unix-abstract':
        # The abstract endpoint is accepted only after the exact bus owner
        # above was tied to the private launcher PID and current UID.
        pass
    return session,address,kind

def verify_desktop(address):
    old={key:os.environ.get(key) for key in ('DBUS_SESSION_BUS_ADDRESS','AT_SPI_BUS','AT_SPI_BUS_ADDRESS')}
    os.environ.update(DBUS_SESSION_BUS_ADDRESS=address,AT_SPI_BUS=address,AT_SPI_BUS_ADDRESS=address)
    try:
        import gi
        gi.require_version('Atspi','2.0')
        from gi.repository import Atspi
        Atspi.init()
        desktop=Atspi.get_desktop(0)
        if desktop is None:
            raise RuntimeError('PRIVATE_ATSPI_DESKTOP_MISSING')
        path=getattr(desktop,'path',None)
        count=desktop.get_child_count()
        if not isinstance(path,str) or not path.startswith('/') or not isinstance(count,int) or count<0:
            raise RuntimeError('PRIVATE_ATSPI_GET_CHILDREN_FAILED')
        return count
    except RuntimeError:
        raise
    except Exception:
        raise RuntimeError('PRIVATE_ATSPI_CLIENT_INIT_FAILED') from None
    finally:
        for key,value in old.items():
            if value is None: os.environ.pop(key,None)
            else: os.environ[key]=value

def prepare_registry(profile):
    if profile.is_symlink() or profile.parent!=Path('/tmp') or not profile.name.startswith('burrata-launch.'):
        raise RuntimeError('PRIVATE_PROFILE_INVALID')
    profile=profile.resolve(strict=True)
    marker=profile/'.burrata-isolated-profile'
    if marker.is_symlink() or marker.read_text() != PROFILE_MARKER:
        raise RuntimeError('PRIVATE_PROFILE_MARKER_INVALID')
    expected_data=(profile/'data').resolve(strict=True)
    if Path(os.environ.get('XDG_DATA_HOME','')).resolve(strict=True)!=expected_data:
        raise RuntimeError('PRIVATE_PROFILE_DATA_HOME_MISMATCH')
    expected_runtime=profile/'runtime'
    if expected_runtime.is_symlink() or not expected_runtime.is_dir() or expected_runtime.resolve(strict=True)!=expected_runtime:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_DIR_INVALID')
    runtime_stat=expected_runtime.stat()
    if runtime_stat.st_uid!=os.getuid() or runtime_stat.st_mode&0o077:
        raise RuntimeError('PRIVATE_ATSPI_RUNTIME_DIR_PERMISSIONS_INVALID')
    service_dir=profile/'atspi-services'
    service=service_dir/'org.a11y.Bus.service'
    config=profile/'atspi-session.conf'
    if (service_dir.is_symlink() or not service_dir.is_dir() or service_dir.stat().st_uid!=os.getuid()
            or service_dir.stat().st_mode&0o077 or service.is_symlink() or not service.is_file()
            or service.stat().st_uid!=os.getuid() or config.is_symlink() or not config.is_file()
            or config.stat().st_uid!=os.getuid()):
        raise RuntimeError('PRIVATE_ATSPI_ACTIVATION_FILES_INVALID')
    if service.read_text()!='[D-BUS Service]\nName=org.a11y.Bus\nExec=/usr/lib/at-spi-bus-launcher --launch-immediately\n':
        raise RuntimeError('PRIVATE_ATSPI_ACTIVATION_SERVICE_INVALID')
    config_text=config.read_text()
    if config_text.count('<servicedir>')!=1 or '<servicedir>'+str(service_dir)+'</servicedir>' not in config_text or '<include' in config_text:
        raise RuntimeError('PRIVATE_ATSPI_ACTIVATION_ALLOWLIST_INVALID')
    session=os.environ.get('DBUS_SESSION_BUS_ADDRESS','')
    if not session.startswith('unix:path=/tmp/dbus-'):
        raise RuntimeError('PRIVATE_SESSION_BUS_INVALID')
    # This call activates the sole service in the private allowlist. The
    # address is not used until its owner is bound to this private runtime.
    session,address,address_kind=call_address(session)
    bus_peer=owned_bus_peer(session,profile)
    if bus_peer is None:
        raise RuntimeError('PRIVATE_ATSPI_BUS_PEER_UNVERIFIED')
    env=os.environ.copy()
    for key in ('DBUS_STARTER_ADDRESS','DBUS_STARTER_BUS_TYPE','DBUS_SYSTEM_BUS_ADDRESS'):
        env.pop(key,None)
    env.update(DBUS_SESSION_BUS_ADDRESS=address,AT_SPI_BUS=address,AT_SPI_BUS_ADDRESS=address,NO_AT_BRIDGE='1')
    env['XDG_RUNTIME_DIR']=str(expected_runtime)
    log=(profile/f'atspi-registry-{os.getpid()}.log').open('wb')
    registry=None
    try:
        registry=subprocess.Popen(['/usr/lib/at-spi2-registryd'],env=env,stdout=log,stderr=subprocess.STDOUT)
        registry_start=proc_start_time(registry.pid)
        owner=False
        for _ in range(100):
            if registry is None or registry.poll() is not None:
                raise RuntimeError('PRIVATE_ATSPI_REGISTRY_EXITED')
            peer=dbus_name_peer(address,'org.a11y.atspi.Registry')
            if (peer and peer['pid']==registry.pid and peer['uid']==os.getuid()
                    and Path(f'/proc/{registry.pid}/exe').resolve(strict=True)==Path('/usr/lib/at-spi2-registryd').resolve(strict=True)):
                owner=True;break
            time.sleep(.05)
        if not owner:
            raise RuntimeError('PRIVATE_ATSPI_REGISTRY_TIMEOUT')
        children=verify_desktop(address)
        if proc_start_time(registry.pid)!=registry_start:
            raise RuntimeError('PRIVATE_ATSPI_REGISTRY_IDENTITY_CHANGED')
        return profile,registry,log,session,address,address_kind,bus_peer,registry_start,children
    except BaseException:
        if registry is not None:
            registry.terminate()
            try:registry.wait(timeout=2)
            except subprocess.TimeoutExpired:registry.kill();registry.wait(timeout=1)
        log.close()
        raise

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--profile',type=Path,required=True)
    parser.add_argument('--preflight-only',action='store_true')
    parser.add_argument('--launch-clock-receipt',type=Path)
    parser.add_argument('command',nargs=argparse.REMAINDER)
    args=parser.parse_args()
    if not args.preflight_only and (not args.command or args.command[0]!='--' or len(args.command)<2):
        parser.error('expected --preflight-only or -- <AppRun>')
    registry=log=None
    try:
        profile,registry,log,session,address,address_kind,bus_peer,registry_start,children=prepare_registry(args.profile)
        if args.preflight_only:
            print(json.dumps({'status':'private_atspi_preflight','registryOwner':True,
                'desktopAccessiblePath':True,'getChildren':True,'desktopChildCount':children,
                'busAddressKind':address_kind,'busOwnerPid':bus_peer['pid'],'busOwnerUid':bus_peer['uid'],
                'busOwnerStartTimeTicks':bus_peer['startTimeTicks'],'registryPid':registry.pid,
                'registryUid':os.getuid(),'registryStartTimeTicks':registry_start},sort_keys=True))
            return 0
        os.environ['DBUS_SESSION_BUS_ADDRESS']=session
        os.environ['AT_SPI_BUS']=address
        os.environ['AT_SPI_BUS_ADDRESS']=address
        os.environ.pop('NO_AT_BRIDGE',None)
        command=args.command[1:]
        if args.launch_clock_receipt:
            if len(command)!=1:raise RuntimeError('PRIVATE_LAUNCH_CLOCK_COMMAND_INVALID')
            image_arg=Path(command[0])
            if image_arg.is_symlink() or not image_arg.is_absolute() or not image_arg.is_file() or not os.access(image_arg,os.X_OK):
                raise RuntimeError('PRIVATE_LAUNCH_CLOCK_IMAGE_INVALID')
            launcher_pid=os.getpid();launcher_start_ticks=proc_start_time(launcher_pid)
            image,image_sha,launch_monotonic_ns,launch_unix_ms=prepare_appimage_launch(image_arg,os.environ)
            app=subprocess.Popen([str(image)])
            try:
                appimage_start_ticks=proc_start_time(app.pid)
                write_launch_clock_receipt(profile,args.launch_clock_receipt,image,image_sha,launcher_pid,launcher_start_ticks,
                    app.pid,appimage_start_ticks,launch_monotonic_ns,launch_unix_ms)
            except BaseException:
                app.terminate()
                try:app.wait(timeout=3)
                except subprocess.TimeoutExpired:app.kill();app.wait(timeout=1)
                raise
        else:
            app=subprocess.Popen(command)
        try:return app.wait()
        finally:
            if app.poll() is None:
                app.terminate()
                try:app.wait(timeout=3)
                except subprocess.TimeoutExpired:app.kill();app.wait(timeout=1)
    except RuntimeError as error:
        reason=str(error)
        if not reason.startswith('PRIVATE_'):reason='PRIVATE_ATSPI_SETUP_FAILED'
        print(json.dumps({'status':'private_atspi_launch_rejected','reasonCode':reason}),file=sys.stderr)
        return 1
    finally:
        if registry is not None:
            if registry.poll() is None: registry.terminate()
            try:registry.wait(timeout=3)
            except subprocess.TimeoutExpired:registry.kill();registry.wait(timeout=1)
        if log is not None:log.close()

if __name__=='__main__': raise SystemExit(main())

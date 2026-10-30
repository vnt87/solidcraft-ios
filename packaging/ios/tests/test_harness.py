"""Portable build-phase contract checks; no Rust compilation or signing required."""
import importlib.util
import plistlib
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class HarnessTests(unittest.TestCase):
    def test_ipados_27_declares_a_single_window_scene(self):
        with (ROOT / 'SolidCraft/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        manifest = info['UIApplicationSceneManifest']
        self.assertFalse(manifest['UIApplicationSupportsMultipleScenes'])
        configurations = manifest['UISceneConfigurations']['UIWindowSceneSessionRoleApplication']
        self.assertEqual(len(configurations), 1)
        self.assertEqual(configurations[0]['UISceneClassName'], 'UIWindowScene')
        self.assertEqual(configurations[0]['UISceneDelegateClassName'], 'SolidCraftSceneDelegate')
        self.assertNotIn('UISceneStoryboardFile', configurations[0])

    def test_rejects_resource_only_bundle(self):
        spec = importlib.util.spec_from_file_location('bundle_validation', ROOT / 'validate-bundle.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory)
            (bundle / 'Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'SolidCraft'}))
            with self.assertRaisesRegex(ValueError, 'Missing bundle executable'):
                module.validate(bundle)
            (bundle / 'SolidCraft').write_text('not a native executable')
            with self.assertRaisesRegex(ValueError, 'not Mach-O'):
                module.validate(bundle)

    def test_bundle_contract(self):
        with (ROOT / 'SolidCraft/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        self.assertEqual(info['CFBundleName'], 'SolidCraft')
        self.assertEqual(info['CFBundleDisplayName'], 'SolidCraft')
        self.assertEqual(info['CFBundleExecutable'], '$(EXECUTABLE_NAME)')
        self.assertEqual(info['CFBundlePackageType'], 'APPL')
        self.assertTrue(info['LSRequiresIPhoneOS'])
        self.assertTrue(info['LSSupportsOpeningDocumentsInPlace'])
        self.assertFalse(info['UIRequiresFullScreen'])
        families = {entry['LSHandlerRank'] for entry in info['CFBundleDocumentTypes']}
        self.assertEqual(families, {'Owner'}, 'the design type is ours')
        exported = {entry['UTTypeIdentifier']: entry for entry in info['UTExportedTypeDeclarations']}
        self.assertIn('io.tangencylabs.solidcraft.design', exported)
        extensions = exported['io.tangencylabs.solidcraft.design']['UTTypeTagSpecification']['public.filename-extension']
        self.assertEqual(extensions[:1], ['sc'], 'new designs are .sc')
        self.assertIn('solidcraft', extensions)

    def test_app_icon_has_no_alpha(self):
        # iOS rejects transparency in an app icon; keep the flattened PNG honest.
        icon = ROOT / 'SolidCraft/Assets.xcassets/AppIcon.appiconset/AppIcon.png'
        self.assertTrue(icon.is_file())
        data = icon.read_bytes()
        self.assertEqual(data[:8], b'\x89PNG\r\n\x1a\n')
        self.assertEqual(data[24], 8, '8 bits per channel')
        self.assertNotIn(data[25], (4, 6), 'PNG colour types 4 and 6 carry an alpha channel')
        with (ROOT / 'SolidCraft/Assets.xcassets/AppIcon.appiconset/Contents.json').open() as source:
            import json
            catalog = json.load(source)
        self.assertEqual(catalog['images'][0]['filename'], 'AppIcon.png')
        self.assertEqual(catalog['images'][0]['size'], '1024x1024')

    def test_build_phase_builds_only_the_static_library(self):
        build = (ROOT / 'build-rust.sh').read_text()
        for expected in ['--package solidcraft-ios', '--lib', 'libsolidcraft_ios.a']:
            self.assertIn(expected, build, expected)
        link = (ROOT / 'link-rust.sh').read_text()
        self.assertIn('-u,_main', link)
        project = (ROOT / 'SolidCraft.xcodeproj/project.pbxproj').read_text()
        self.assertIn('io.tangencylabs.solidcraft', project)
        for script in ['build-rust.sh', 'link-rust.sh']:
            self.assertIn(script, project, script)
            self.assertTrue((ROOT / script).stat().st_mode & 0o111, f'{script} must be executable')

    def test_deployment_floor_and_device_family(self):
        project = (ROOT / 'SolidCraft.xcodeproj/project.pbxproj').read_text()
        self.assertIn('IPHONEOS_DEPLOYMENT_TARGET = 17.0;', project)
        self.assertIn('TARGETED_DEVICE_FAMILY = 2;', project, 'iPad only')

    def test_status_bar_is_hidden_for_the_shell(self):
        # winit's view controller hides it; the plist keeps the same answer if UIKit asks the app.
        with (ROOT / 'SolidCraft/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        self.assertFalse(info['UIViewControllerBasedStatusBarAppearance'])
        self.assertTrue(info['UIStatusBarHidden'])

    def test_relative_cargo_target_dir_is_resolved_once(self):
        # Xcode runs build phases inside packaging/ios, where a relative CARGO_TARGET_DIR would
        # create a second multi-gigabyte target tree; it must resolve from the repository root.
        build = (ROOT / 'build-rust.sh').read_text()
        self.assertIn('${repository_root}/${CARGO_TARGET_DIR}', build)


if __name__ == '__main__':
    unittest.main()

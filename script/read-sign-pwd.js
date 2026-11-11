#!/usr/bin/env node
"use strict";

// Reads the keystore password of a HAP project out of the encrypted value that
// DevEco Studio keeps in build-profile.json5.
//
// The scheme is not a secret: hvigor ships it in plain JavaScript at
// hvigor-ohos-plugin/src/utils/decipher-util.js. Only the random material under
// the keystore's sibling `material/` directory is per-keystore, and that
// directory travels with the keystore, so the password stays recoverable
// locally. Doing this at build time means swapping a keystore never requires
// syncing the password into this repository by hand.
//
// Usage: node read-sign-pwd.js <hap-project-dir> [--config <name>] [--product <name>] [--material]
//   (no flag)   prints the keystore password
//   --material  prints certpath, profile, storeFile, keyAlias and the config name
// Without --config the product's own signingConfig is used.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

// The fixed 16-byte component from hvigor's decipher-util.js. Useless on its
// own: the per-keystore randomness lives in material/.
const COMPONENT = new Int8Array([
  49, 243, 9, 115, 214, 175, 91, 184, 211, 190, 177, 88, 101, 131, 192, 119,
]);

const PROFILE_NAME = "build-profile.json5";

function fail(message) {
  process.stderr.write("read-sign-pwd: " + message + "\n");
  process.exit(1);
}

function readField(profileText, field, quoted) {
  const pattern = quoted
    ? new RegExp('"' + field + '"\\s*:\\s*"([^"]+)"')
    : new RegExp('"' + field + '"\\s*:\\s*"([0-9a-fA-F]+)"');
  const match = pattern.exec(profileText);
  if (!match) {
    fail(`no "${field}" found in ${PROFILE_NAME}`);
  }
  return match[1];
}

function readSingleFile(dir) {
  const names = fs.readdirSync(dir).filter((name) => name !== ".DS_Store");
  if (names.length !== 1) {
    fail(`expected exactly one file under ${dir}, found ${names.length}`);
  }
  return new Int8Array(fs.readFileSync(path.resolve(dir, names[0])));
}

function xorAll(parts) {
  const length = parts[0].byteLength;
  const out = new Int8Array(length);
  for (const part of parts) {
    if (part.byteLength !== length) {
      fail("key material component length mismatch");
    }
    for (let i = 0; i < length; i++) {
      out[i] ^= part[i];
    }
  }
  return Buffer.from(out);
}

function openGcm(key, data) {
  const declared =
    ((255 & data[0]) << 24) | ((255 & data[1]) << 16) | ((255 & data[2]) << 8) | (255 & data[3]);
  const ivLength = data.length - 4 - declared;
  const decipher = crypto.createDecipheriv("aes-128-gcm", key, data.slice(4, 4 + ivLength));
  decipher.setAuthTag(data.slice(data.length - 16));
  const head = decipher.update(data.subarray(4 + ivLength, data.length - 16));
  return Buffer.concat([head, decipher.final()]);
}

function unwrapDataKey(materialRoot) {
  const shardRoot = path.resolve(materialRoot, "fd");
  const shards = fs
    .readdirSync(shardRoot)
    .filter((name) => name !== ".DS_Store")
    .map((name) => readSingleFile(path.resolve(shardRoot, name)));
  const salt = readSingleFile(path.resolve(materialRoot, "ac"));
  const rootKey = new Int8Array(
    crypto.pbkdf2Sync(xorAll(shards.concat([COMPONENT])).toString(), salt, 10000, 16, "sha256"),
  );
  return openGcm(rootKey, readSingleFile(path.resolve(materialRoot, "ce")));
}

const projectDir = process.argv[2];
if (!projectDir) {
  fail("usage: node read-sign-pwd.js <hap-project-dir> [--config <name>] [--product <name>] [--material]");
}

const profilePath = path.resolve(projectDir, PROFILE_NAME);
let profileText;
try {
  profileText = fs.readFileSync(profilePath, "utf-8");
} catch (error) {
  fail(`cannot read ${profilePath}: ${error.message}`);
}

// build-profile.json5 is JSON5: hvigor writes it, and DevEco Studio may leave
// comments or trailing commas behind. Strip those before JSON.parse so this
// script is never stricter than the toolchain that produced the file.
function stripJson5(text) {
  let out = "";
  let inString = false;
  let quote = "";
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (inString) {
      out += ch;
      if (ch === "\\") {
        out += text[i + 1] || "";
        i += 1;
      } else if (ch === quote) {
        inString = false;
      }
      continue;
    }
    if (ch === '"' || ch === "'") {
      inString = true;
      quote = ch;
      out += ch;
      continue;
    }
    if (ch === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i += 1;
      out += "\n";
      continue;
    }
    if (ch === "/" && text[i + 1] === "*") {
      i += 2;
      while (i < text.length && !(text[i] === "*" && text[i + 1] === "/")) i += 1;
      i += 1;
      continue;
    }
    out += ch;
  }
  return out.replace(/,(\s*[}\]])/g, "$1");
}

function parseProfile(text) {
  try {
    return JSON.parse(stripJson5(text));
  } catch {
    return null;
  }
}

function pickConfig(profile, configName, productName) {
  const app = profile.app || {};
  const configs = app.signingConfigs || [];
  const products = app.products || [];
  const product =
    (productName ? products.find((entry) => entry.name === productName) : null) ||
    products[0] ||
    {};
  const wanted = configName || product.signingConfig;
  const config = wanted ? configs.find((entry) => entry.name === wanted) : configs[0];
  if (!config) {
    fail("no signingConfig \"" + (wanted || "") + "\" in " + PROFILE_NAME);
  }
  return config;
}

function readOptions(argv) {
  const options = { config: "", product: "", material: false };
  for (let i = 3; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--config") {
      options.config = argv[i + 1] || "";
      i += 1;
    } else if (arg === "--product") {
      options.product = argv[i + 1] || "";
      i += 1;
    } else if (arg === "--material") {
      options.material = true;
    } else if (!arg.startsWith("--") && !options.config) {
      // Older call sites passed the config name positionally.
      options.config = arg;
    } else {
      fail("unknown argument: " + arg);
    }
  }
  return options;
}

const options = readOptions(process.argv);
const profile = parseProfile(profileText);
if (!profile) {
  fail(PROFILE_NAME + " is not parseable as JSON5; pass OHOS_SIGN_* overrides instead");
}
const config = pickConfig(profile, options.config, options.product);
const material = config.material || {};

if (options.material) {
  const values = ["certpath", "profile", "storeFile", "keyAlias"].map((name) => {
    const value = material[name];
    if (!value) {
      fail("signingConfig \"" + config.name + "\" has no \"" + name + "\"");
    }
    return value;
  });
  process.stdout.write(values.concat([config.name]).join("\n") + "\n", () => {
    process.exit(0);
  });
} else {
  const encrypted = material.storePassword;
  const storeFile = material.storeFile;
  if (!encrypted || !storeFile) {
    fail("signingConfig \"" + config.name + "\" has no storePassword/storeFile");
  }
  const materialRoot = path.resolve(
    path.dirname(path.resolve(projectDir, storeFile)),
    "material",
  );
  if (!fs.existsSync(materialRoot)) {
    fail("no material directory at " + materialRoot + "; cannot recover the password");
  }
  const plain = openGcm(
    unwrapDataKey(materialRoot),
    new Int8Array(Buffer.from(encrypted, "hex")),
  ).toString("utf-8");
  if (plain.length === 0) {
    fail("recovered an empty password");
  }
  process.stdout.write(plain + "\n");
}

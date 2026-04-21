# Changelog

## [0.2.0](https://github.com/jguice/utter/compare/v0.1.0...v0.2.0) (2026-04-21)


### Features

* add 'utter set-key' for interactive PTT key binding ([02e1ba6](https://github.com/jguice/utter/commit/02e1ba65a9eddd2e29853afb8f1b58261b937af9))
* **clipboard:** add UTTER_CLIPBOARD=1 opt-in for regular clipboard write ([bfa7cf1](https://github.com/jguice/utter/commit/bfa7cf1b6f931ddfee2e8808d72f7e4eb4d688f6))
* **set-key:** accept any evdev key; expand canonical aliases ([29fd2a3](https://github.com/jguice/utter/commit/29fd2a36fd56982aac5020dec90f3a5b51c9618e))
* utter set-key, paste cleanup, CI test + coverage gates ([3313b9c](https://github.com/jguice/utter/commit/3313b9c0d0c9ce545ea1b394a136e0b7f7ea2b68))
* **version:** include git SHA in --version output ([fbc9eb6](https://github.com/jguice/utter/commit/fbc9eb6f364f4e9f38cfe1c695c9e8505dea2a52))


### Bug Fixes

* **install:** stop services and force-install to avoid ETXTBSY on upgrade ([14fa212](https://github.com/jguice/utter/commit/14fa2129aaf23ec99aba4d1b9810ad9500643c07))
* **paste:** restore ydotool's 12ms key-delay for modifier chords ([3edb3f9](https://github.com/jguice/utter/commit/3edb3f9178bc76d12537b4ebe306fa8c51a01d2d))
* **set-key:** always activate watcher with new config after save ([08ff95b](https://github.com/jguice/utter/commit/08ff95b5e8b1bc9bffe17267075c56046c5c3dfe))
* **set-key:** report actual watcher state, don't promise what we can't verify ([8363512](https://github.com/jguice/utter/commit/8363512b0af7ce52acc581fd6ce1d553ae03e34c))
* **uninstall:** reap orphan processes and stale sockets after package removal ([d7f611b](https://github.com/jguice/utter/commit/d7f611b72ccee91c5eaebcbd8625af93b9b31538))


### Refactoring

* **paste:** primary-only clipboard, drop unused paste method alternates ([32e7106](https://github.com/jguice/utter/commit/32e7106f511e9575f0ecd1e4212377085400f060))


### Documentation

* also document in the README that the OS microphone-in-use ([02e1ba6](https://github.com/jguice/utter/commit/02e1ba65a9eddd2e29853afb8f1b58261b937af9))
* **backlog:** update stale pre-rename version references ([0d98d69](https://github.com/jguice/utter/commit/0d98d6907f2b17be7de0b89daff4cf07f6dd8ef9))
* clarify which service owns which configuration ([63e185c](https://github.com/jguice/utter/commit/63e185c9209d3bebf40b27def7653de3f70316b2))
* clarity pass on README ([8023969](https://github.com/jguice/utter/commit/80239693322a885d7790b60155882ae62750d988))
* lead README with what utter does, not what other tools lack ([0302453](https://github.com/jguice/utter/commit/03024533d0a45efec87e05822ad3ad9be275f1ce))
* **redeploy:** use `cp --remove-destination` to avoid ETXTBSY ([6a3ee73](https://github.com/jguice/utter/commit/6a3ee73d5838cea421d99da9dc7a9fcfa7c8fdca))

## Changelog

This file is maintained automatically by [release-please](https://github.com/googleapis/release-please) from [conventional commits](https://www.conventionalcommits.org/). Do not edit by hand.

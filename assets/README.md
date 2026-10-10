# Embedded sound samples

`bucklespring.pack` contains the IBM Model M keyboard recordings from
[bucklespring](https://github.com/zevv/bucklespring) by Ico Doornekamp
(Copyright 2016), licensed under the GNU GPL version 2. They are converted to
mono and packed into a single file by `tools/pack_bucklespring.py`, and
embedded in the heptix binary.

Regenerate with:

```sh
git clone https://github.com/zevv/bucklespring
python3 tools/pack_bucklespring.py bucklespring/wav assets/bucklespring.pack
```

`wheel_of_time.caf` is a short recorded tick used as a built-in scroll sound.

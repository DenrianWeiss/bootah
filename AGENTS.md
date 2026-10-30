# Bootah

## About the project

See [README](README.md)

## Development Requirements

### General


- Do NOT add platform specific code unless it is really nessary, or you are developing shell for the platform.
- Make sure to re-use previously implemented component if possible.
- Take care of i18n support
- After finishing certain feature, test and commit it, update related docs around it.

### Adding extra support

- When adding protocol or OS support, refer to the vendor document instead of guessing, after finish your development, run the test on emulated or real host. If nessary, capture the packet and compare with other implementations.
- Check license before introducing external dependency.

### Debugging

- Use captured packet instead of guessing, and try to compare with vendor products, to make sure you have do it right. Also, use debugger instead of guessing or adding time when timeout ocurred.

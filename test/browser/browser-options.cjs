const fs = require('node:fs');
module.exports = {
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
    : fs.existsSync('/usr/bin/chromium')
      ? { executablePath: '/usr/bin/chromium' }
      : {}),
};

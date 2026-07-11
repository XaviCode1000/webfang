---
title: web-scraping.dev File Download
url: https://web-scraping.dev/file-download
date: 2026-07-11
excerpt: 'This scenario demonstrates a form submission that downloads a file with Content-Disposition: attachment header in a new tab.'
---

This scenario demonstrates a form submission that downloads a file with `Content-Disposition: attachment` header in a new tab.

##### Download Sample File

Click the button below to submit a form via POST request. The response will be a PDF file that downloads automatically.

###### Technical Details:

*   **Method:** POST
*   **Endpoint:** `/api/download-file`
*   **Button ID:** `download-btn`
*   **Target:** `_blank` (opens in new tab)
*   **Response Header:** `Content-Disposition: attachment; filename=download-sample.pdf`
*   **Content Type:** `application/pdf`

**For Web Scrapers:** This pattern is commonly used for generating dynamic reports, invoices, or exports. You'll need to handle the form POST request and capture the file response appropriately. 

#### Code Examples

Choose your preferred method for downloading files:

▶ Method 1: Direct API Request (Python Requests) 

Simplest approach - directly POST to the API endpoint without a browser.

```
import requests # Submit the form response = requests.post( "http://web-scraping.dev/api/download-file", headers={"Content-Type": "application/x-www-form-urlencoded"} ) # Save the PDF file if response.status_code == 200: with open("downloaded_file.pdf", "wb") as f: f.write(response.content) print("File downloaded successfully!") 
```

▶ Method 2: Playwright (Python) 

Modern browser automation with built-in download handling. Recommended for headless automation.

```
from playwright.sync_api import sync_playwright with sync_playwright() as p: browser = p.chromium.launch(headless=True) page = browser.new_page() # Navigate to the download page page.goto("http://web-scraping.dev/file-download") # Set up download handler before clicking with page.expect_download() as download_info: # Click the download button (ID: download-btn) page.click("#download-btn") # Wait for download and save it download = download_info.value download.save_as("downloaded_file.pdf") print(f"Downloaded: {download.suggested_filename}") browser.close() 
```

▶ Method 3: Selenium with Chrome DevTools Protocol (Python) 

Configure Chrome to automatically download PDFs to a specific directory.

```
from selenium import webdriver from selenium.webdriver.common.by import By from selenium.webdriver.chrome.service import Service from selenium.webdriver.chrome.options import Options import time import os # Set up Chrome options chrome_options = Options() download_dir = os.path.abspath("./downloads") os.makedirs(download_dir, exist_ok=True) prefs = { "download.default_directory": download_dir, "download.prompt_for_download": False, "plugins.always_open_pdf_externally": True } chrome_options.add_experimental_option("prefs", prefs) # Initialize driver driver = webdriver.Chrome(options=chrome_options) # Navigate and download driver.get("http://web-scraping.dev/file-download") # Click the download button using its ID download_button = driver.find_element(By.ID, "download-btn") download_button.click() # Wait for download to complete time.sleep(3) # Check downloaded file files = os.listdir(download_dir) if files: print(f"Downloaded: {files[0]}") driver.quit() 
```

▶ Method 4: cURL with ScrapFly API 

Use ScrapFly's API to download the file via HTTP request with automatic anti-bot bypassing.

```
# Download file via ScrapFly API curl -X POST "https://api.scrapfly.io/scrape" \ -H "Content-Type: application/json" \ -d '{ "key": "YOUR_API_KEY", "url": "https://web-scraping.dev/api/download-file", "method": "POST" }' | jq -r '.result.content' | base64 -d > downloaded_file.pdf echo "File downloaded successfully!" 
```

**Note:** Sign up at [scrapfly.io](https://scrapfly.io/) to get your API key. Free tier includes 1,000 API credits/month. 

▶ Method 5: Puppeteer (Node.js) 

Node.js browser automation using Chrome DevTools Protocol.

```
const puppeteer = require('puppeteer'); const fs = require('fs'); (async () => { const browser = await puppeteer.launch({ headless: true }); const page = await browser.newPage(); // Enable download interception const client = await page.target().createCDPSession(); await client.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: './downloads' }); await page.goto('http://web-scraping.dev/file-download'); // Click the download button by ID await page.click('#download-btn'); // Wait for download (adjust timeout as needed) await page.waitForTimeout(3000); console.log('Download completed!'); await browser.close(); })(); 
```

▶ Method 6: ScrapFly Python SDK 

Using ScrapFly Python SDK to download files with automatic proxy rotation and anti-bot protection.

```
from scrapfly import ScrapflyClient, ScrapeConfig # Initialize ScrapFly client client = ScrapflyClient(key='YOUR_API_KEY') # Download the file via POST request result = client.scrape(ScrapeConfig( url='https://web-scraping.dev/api/download-file', method='POST', render_js=False, )) # Save the downloaded file with open('downloaded_file.pdf', 'wb') as f: f.write(result.content.encode('latin-1')) print("File downloaded successfully!") 
```

Install the SDK: `pip install scrapfly-sdk`
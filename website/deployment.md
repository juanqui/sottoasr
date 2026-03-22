# Sotto Website — Cloudflare Pages Deployment

## Overview

The Sotto website is a static site (plain HTML/CSS) hosted on **Cloudflare Pages** using a custom domain purchased through Cloudflare. Total cost: **$0** on the free tier.

## What's in the Free Tier

| Resource              | Limit         |
|-----------------------|---------------|
| Bandwidth             | Unlimited     |
| Static asset requests | Unlimited     |
| Projects              | Unlimited     |
| Builds per month      | 500           |
| Concurrent builds     | 1             |
| Files per deployment  | 20,000        |
| Max file size         | 25 MB         |
| Custom domains        | 100/project   |
| SSL/TLS               | Automatic     |

## Setup Steps

### 1. Create the Pages Project

1. Log in to the [Cloudflare dashboard](https://dash.cloudflare.com/)
2. Go to **Workers & Pages** > **Create** > **Pages** > **Connect to Git**
3. Select the **sotto** repository from GitHub
4. Configure the build settings:
   - **Production branch:** `main`
   - **Build command:** *(leave empty — no build step needed)*
   - **Build output directory:** `website`
5. Click **Save and Deploy**

### 2. Connect the Custom Domain

1. Go to **Workers & Pages** > your project > **Custom domains**
2. Click **Set up a domain**
3. Enter your domain (e.g., `sotto.app`)
4. Since the domain is already on Cloudflare, DNS records are created automatically
5. SSL is provisioned automatically — no manual certificate setup

To also serve from `www`:
- Add `www.sotto.app` as a second custom domain
- Optionally set up a redirect rule to canonicalize to one or the other

### 3. Verify

- Visit your domain over HTTPS and confirm the site loads
- Push a change to `main` and confirm Cloudflare auto-deploys it

## How Deployments Work

- Every push to `main` triggers a **production deployment** automatically
- Every push to any other branch creates a **preview deployment** with a unique URL
- Preview URLs are posted on pull requests for easy review
- No GitHub Actions or CI configuration is needed — Cloudflare handles everything

## Project Structure

```
website/
├── index.html      # Landing page
├── style.css       # Styles
├── assets/
│   └── logo.png    # Logo
└── deployment.md   # This file
```

## Notes

- The website is completely separate from the Tauri app build (`npm run build` does not touch `website/`)
- Cloudflare is gradually merging Pages into Workers with Static Assets, but Pages remains fully supported — no action needed unless Cloudflare announces a migration timeline
- If the site ever needs a build step (e.g., adding a framework), just update the build command in the Pages dashboard

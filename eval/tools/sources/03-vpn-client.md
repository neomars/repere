---
title: "Client VPN : installation, connexion et dépannage"
keywords: [vpn, accès distant, télétravail, erreur 809, erreur 691]
lang: fr-FR
---

# Présentation

Le VPN (réseau privé virtuel) crée un tunnel chiffré entre votre ordinateur et le réseau de la collectivité. Il est nécessaire pour accéder aux dossiers partagés, à l'intranet et aux applications métier depuis l'extérieur, par exemple en télétravail ou en déplacement.

# Installer le client VPN

Le client VPN est préinstallé sur les ordinateurs portables. S'il manque, installez-le depuis le Centre logiciel :

1. Ouvrez le Centre logiciel depuis le menu Démarrer.
2. Recherchez « Client VPN » puis cliquez sur Installer.
3. Redémarrez l'ordinateur lorsque l'installation est terminée.

# Se connecter au VPN

1. Cliquez sur l'icône du client VPN, près de l'horloge de la barre des tâches.
2. Choisissez le profil « Collectivité » puis cliquez sur Connecter.
3. Saisissez votre identifiant et votre mot de passe de session.
4. Validez la demande de connexion dans l'application Authenticator de votre téléphone.

Une fois la connexion établie, l'icône affiche un cadenas. Vous pouvez alors utiliser les lecteurs réseau et l'intranet comme au bureau.

# Les erreurs courantes du client VPN

## Erreur 809

L'erreur 809 indique que la connexion est bloquée avant d'atteindre la collectivité, le plus souvent par un pare-feu ou par la box Internet. Pour la résoudre :

1. Autorisez les ports UDP 500 et UDP 4500 dans le pare-feu ou sur la box Internet.
2. Si vous êtes sur un réseau d'hôtel, de gare ou d'entreprise, ces ports sont souvent bloqués : utilisez le partage de connexion de votre téléphone.
3. Relancez le client VPN.

Si l'erreur 809 persiste, ouvrez un ticket en précisant le réseau utilisé (domicile, hôtel, partage de connexion).

## Erreur 691

L'erreur 691 signifie que l'identifiant ou le mot de passe est refusé. Vérifiez que la touche de verrouillage des majuscules n'est pas active et que votre compte n'est pas verrouillé. Si vous avez changé votre mot de passe récemment, utilisez le nouveau mot de passe.

## Erreur 720

Le profil de connexion est corrompu. Désinstallez le client VPN depuis le Centre logiciel, redémarrez, puis réinstallez-le.

# Tableau des codes d'erreur

| Code | Cause probable | Solution |
|------|----------------|----------|
| 809 | Pare-feu ou box Internet | Ouvrir les ports UDP 500 et 4500 |
| 691 | Identifiant ou mot de passe refusé | Vérifier le mot de passe et le verrouillage du compte |
| 720 | Profil corrompu | Réinstaller le client VPN |

# Connecté au VPN mais sans accès aux dossiers

Ouvrez une invite de commandes, tapez ipconfig /flushdns puis validez, et reconnectez le lecteur réseau. Si cela ne suffit pas, déconnectez puis reconnectez le VPN.

# Le VPN est lent

Fermez les visioconférences inutiles et préférez un câble réseau au Wi-Fi domestique. N'ouvrez pas de gros fichiers directement depuis le lecteur réseau : copiez-les d'abord sur votre ordinateur.
